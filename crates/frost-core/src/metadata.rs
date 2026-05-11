//! Iceberg metadata types used by frost health checks.
//!
//! These are frost's own representations of the metadata concepts we need.
//! They decouple check logic from any specific Iceberg library, making it
//! easy to test with fixtures and to swap out the metadata source later
//! (e.g., switching from hand-parsed JSON to iceberg-rust).

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Complete metadata snapshot for a single Iceberg table.
/// This is the primary input to all health checks.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TableMetadata {
    /// Fully qualified table name (e.g., "db.events").
    pub table_name: String,
    /// Location of the table (e.g., "s3://bucket/warehouse/db/events").
    pub location: String,
    /// Iceberg format version (1 or 2).
    #[serde(default = "default_format_version")]
    pub format_version: i32,
    /// Table UUID, if present in metadata.
    #[serde(default)]
    pub table_uuid: Option<String>,
    /// Table properties (e.g., `write.target-file-size-bytes`,
    /// `write.distribution-mode`, `format-version`). Unknown properties
    /// are kept as-is.
    #[serde(default)]
    pub properties: HashMap<String, String>,
    /// Current schema of the table.
    pub current_schema: Schema,
    /// All schema versions (for schema history check).
    pub schemas: Vec<Schema>,
    /// All snapshots, ordered oldest to newest.
    pub snapshots: Vec<Snapshot>,
    /// Current snapshot ID, if any.
    pub current_snapshot_id: Option<i64>,
    /// Default partition spec.
    pub partition_spec: PartitionSpec,
    /// Every partition spec the table has ever had — used to detect
    /// partition-spec-evolution churn. Includes the default spec.
    #[serde(default)]
    pub partition_specs: Vec<PartitionSpec>,
    /// Default sort order, if declared.
    pub sort_order: Option<SortOrder>,
    /// Every sort order the table has ever had.
    #[serde(default)]
    pub sort_orders: Vec<SortOrder>,
    /// Named refs (branches and tags). The `main` branch is always present
    /// on a table that has at least one snapshot; other entries indicate
    /// the table is using Iceberg branching.
    #[serde(default)]
    pub refs: HashMap<String, SnapshotRef>,
    /// All data files referenced by the current snapshot.
    pub data_files: Vec<DataFile>,
    /// Delete files (position or equality deletes) in the current snapshot.
    pub delete_files: Vec<DeleteFile>,
    /// All file paths found in the table's data directory (for orphan detection).
    pub all_storage_paths: Vec<String>,
    /// Total size of metadata files (snapshot JSON + manifest lists + manifests) in bytes.
    pub metadata_size_bytes: u64,
    /// Per-manifest stats: count, total size, distribution. Used for the
    /// metadata_size and properties_drift checks. Empty if manifests
    /// were not loaded.
    #[serde(default)]
    pub manifest_stats: ManifestStats,
    /// Timestamp when this metadata was collected.
    pub collected_at: DateTime<Utc>,
}

fn default_format_version() -> i32 {
    2
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Schema {
    pub schema_id: i32,
    pub fields: Vec<Field>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Field {
    pub id: i32,
    pub name: String,
    pub field_type: String,
    pub required: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Snapshot {
    pub snapshot_id: i64,
    /// Parent snapshot ID — useful for chain-of-changes analysis.
    #[serde(default)]
    pub parent_snapshot_id: Option<i64>,
    pub timestamp_ms: i64,
    /// Operation type pulled from the summary (`append`, `overwrite`,
    /// `delete`, `replace`). Different operations bloat metadata at
    /// different rates.
    #[serde(default)]
    pub operation: Option<String>,
    pub summary: HashMap<String, String>,
    pub manifest_list: String,
    /// Schema ID active when this snapshot was committed.
    #[serde(default)]
    pub schema_id: Option<i32>,
}

impl Snapshot {
    pub fn timestamp(&self) -> DateTime<Utc> {
        DateTime::from_timestamp_millis(self.timestamp_ms).unwrap_or_default()
    }
}

/// A named ref (branch or tag) pointing at a specific snapshot. Iceberg v2.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapshotRef {
    pub snapshot_id: i64,
    /// "branch" or "tag".
    #[serde(rename = "type")]
    pub ref_type: String,
    /// Optional retention: snapshots older than this many ms are eligible
    /// for expiration on this branch.
    #[serde(default)]
    pub max_ref_age_ms: Option<i64>,
    #[serde(default)]
    pub max_snapshot_age_ms: Option<i64>,
    #[serde(default)]
    pub min_snapshots_to_keep: Option<i32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PartitionSpec {
    pub spec_id: i32,
    pub fields: Vec<PartitionField>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PartitionField {
    pub source_id: i32,
    pub field_id: i32,
    pub name: String,
    pub transform: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SortOrder {
    pub order_id: i32,
    pub fields: Vec<SortField>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SortField {
    pub source_id: i32,
    pub transform: String,
    pub direction: String,
    pub null_order: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DataFile {
    pub file_path: String,
    pub file_size_bytes: u64,
    pub record_count: u64,
    /// Partition values as key-value pairs.
    pub partition: HashMap<String, String>,
    pub file_format: FileFormat,
    /// Column statistics from the manifest entry. Empty maps mean "not
    /// recorded" — different engines vary in how aggressively they write
    /// these. Available column IDs depend on writer settings.
    #[serde(default)]
    pub column_sizes: HashMap<i32, i64>,
    #[serde(default)]
    pub value_counts: HashMap<i32, i64>,
    #[serde(default)]
    pub null_value_counts: HashMap<i32, i64>,
    /// Sort order ID this file was written under (Iceberg v2 only).
    /// `None` means the file does not declare a sort order.
    #[serde(default)]
    pub sort_order_id: Option<i32>,
    /// Spec ID this file was partitioned under. Useful for detecting
    /// partition-spec evolution churn (files written under old specs).
    #[serde(default)]
    pub spec_id: Option<i32>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DeleteFile {
    pub file_path: String,
    pub file_size_bytes: u64,
    pub record_count: u64,
    pub delete_type: DeleteType,
    /// For equality deletes: the column field IDs the delete predicate
    /// applies to. Empty for position deletes.
    #[serde(default)]
    pub equality_ids: Vec<i32>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FileFormat {
    #[default]
    Parquet,
    Avro,
    Orc,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeleteType {
    #[default]
    PositionDelete,
    EqualityDelete,
}

/// Aggregate manifest statistics collected when manifests are loaded.
/// Powers the metadata_size and properties_drift checks.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ManifestStats {
    /// Number of manifest files referenced from the current snapshot.
    pub manifest_count: u64,
    /// Total size in bytes (sum of `manifest_length` across the manifest list).
    pub manifests_total_bytes: u64,
    /// Median manifest size in bytes.
    pub median_manifest_bytes: u64,
    /// Largest manifest in bytes.
    pub max_manifest_bytes: u64,
}
