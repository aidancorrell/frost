use serde::{Deserialize, Serialize};

/// Top-level frost configuration, typically loaded from frost.toml.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct FrostConfig {
    pub catalog: CatalogConfig,
    pub thresholds: Thresholds,
    pub cost: CostConfig,
    pub output: OutputConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum CatalogConfig {
    Rest {
        uri: String,
        warehouse: String,
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

impl Default for FrostConfig {
    fn default() -> Self {
        Self {
            catalog: CatalogConfig::default(),
            thresholds: Thresholds::default(),
            cost: CostConfig::default(),
            output: OutputConfig::default(),
        }
    }
}

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
        }
    }
}

impl Default for CostConfig {
    fn default() -> Self {
        Self {
            s3_storage_per_gb_month: 0.023,
            s3_get_request_per_1000: 0.0004,
            region: "us-east-1".to_string(),
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
