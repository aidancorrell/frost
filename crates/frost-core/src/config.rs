use crate::watch::WatchConfig;
use serde::{Deserialize, Serialize};

/// Top-level frost configuration, typically loaded from frost.toml.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct FrostConfig {
    pub catalog: CatalogConfig,
    pub thresholds: Thresholds,
    pub cost: CostConfig,
    pub output: OutputConfig,
    pub watch: WatchConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum CatalogConfig {
    Rest {
        uri: String,
        warehouse: String,
        #[serde(default)]
        token: Option<String>,
        #[serde(default)]
        prefix: Option<String>,
    },
    Glue {
        region: Option<String>,
        warehouse: String,
    },
    Filesystem {
        warehouse: String,
    },
}

/// Thresholds for health checks. All values are configurable per-org.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Thresholds {
    /// Files smaller than this (in bytes) are flagged as "small files". Default: 8 MB.
    pub small_file_bytes: u64,
    /// Maximum number of snapshots before flagging bloat. Default: 100.
    pub max_snapshots: u64,
    /// Maximum age of oldest snapshot in days. Default: 7.
    pub max_snapshot_age_days: u64,
    /// Flag if any partition has more than this ratio × median file count. Default: 10.0.
    pub partition_skew_ratio: f64,
    /// Maximum outstanding delete files before flagging. Default: 50.
    pub max_delete_files: u64,
    /// Hours since last commit before a table is considered stale. Default: 48.
    pub stale_table_hours: u64,
    /// Maximum total metadata size in bytes before flagging. Default: 500 MB.
    pub max_metadata_bytes: u64,
    /// Number of distinct partition specs (incl. retired ones) before
    /// flagging spec-evolution churn. Default: 3.
    pub max_partition_specs: usize,
    /// Minimum % of data files (0–100) that must declare a sort order ID
    /// matching the table's default. Below this, sort_compliance flags.
    /// Default: 80.
    pub min_sort_compliance_pct: f64,
    /// Minimum % of data files that must report at least one column's
    /// lower/upper bounds (or value_counts) for stats_coverage to pass.
    /// Default: 50.
    pub min_stats_coverage_pct: f64,
    /// Branch is considered stale if its target snapshot was committed
    /// more than this many days ago. Default: 30.
    pub stale_branch_days: u64,
    /// Variance allowed between `write.target-file-size-bytes` and the
    /// observed median data file size before flagging properties_drift.
    /// Default: 50% (0.5).
    pub target_file_size_drift_pct: f64,
}

/// S3 pricing model for cost estimation.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct CostConfig {
    /// Cost per GB per month for S3 Standard storage.
    pub s3_storage_per_gb_month: f64,
    /// Cost per 1000 GET requests.
    pub s3_get_request_per_1000: f64,
    /// AWS region (for pricing context).
    pub region: String,
    /// Assumed queries-per-day issued against this table. Used to scale
    /// small-files and metadata-size cost estimates. Make this match
    /// reality for your workload — defaults to 100.
    pub queries_per_day: f64,
    /// Compute cost per CPU-hour for the engine that runs queries
    /// against this table. Used to model scan-time overhead from delete
    /// files and small-file planning. Defaults to $0.05/CPU-hour
    /// (rough mid-point of EMR/spot pricing).
    pub compute_cost_per_cpu_hour: f64,
    /// Average bytes scanned per query against this table. Used to
    /// estimate the overhead delete files add (a percentage of normal
    /// scan time). Defaults to 1 GB.
    pub avg_bytes_scanned_per_query: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct OutputConfig {
    /// Default output format: "pretty", "json", or "github-actions".
    pub default_format: OutputFormat,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum OutputFormat {
    Pretty,
    Json,
    GithubActions,
}

// --- Defaults ---

impl Default for CatalogConfig {
    fn default() -> Self {
        Self::Filesystem {
            warehouse: "./warehouse".to_string(),
        }
    }
}

impl Default for Thresholds {
    fn default() -> Self {
        Self {
            small_file_bytes: 8 * 1024 * 1024, // 8 MB
            max_snapshots: 100,
            max_snapshot_age_days: 7,
            partition_skew_ratio: 10.0,
            max_delete_files: 50,
            stale_table_hours: 48,
            max_metadata_bytes: 500 * 1024 * 1024, // 500 MB
            max_partition_specs: 3,
            min_sort_compliance_pct: 80.0,
            min_stats_coverage_pct: 50.0,
            stale_branch_days: 30,
            target_file_size_drift_pct: 0.5,
        }
    }
}

impl Default for CostConfig {
    fn default() -> Self {
        Self {
            s3_storage_per_gb_month: 0.023,
            s3_get_request_per_1000: 0.0004,
            region: "us-east-1".to_string(),
            queries_per_day: 100.0,
            compute_cost_per_cpu_hour: 0.05,
            avg_bytes_scanned_per_query: 1024 * 1024 * 1024,
        }
    }
}

impl Default for OutputConfig {
    fn default() -> Self {
        Self {
            default_format: OutputFormat::Pretty,
        }
    }
}

impl FrostConfig {
    /// Load config from a TOML file, falling back to defaults for missing fields.
    pub fn from_file(path: &std::path::Path) -> Result<Self, ConfigError> {
        let contents = std::fs::read_to_string(path).map_err(ConfigError::Io)?;
        let config: FrostConfig = toml::from_str(&contents).map_err(ConfigError::Parse)?;
        Ok(config)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("failed to read config file: {0}")]
    Io(#[from] std::io::Error),
    #[error("failed to parse config: {0}")]
    Parse(#[from] toml::de::Error),
}
