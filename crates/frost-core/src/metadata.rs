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
    /// Current schema of the table.
    pub current_schema: Schema,
    /// All schema versions (for schema history check).
    pub schemas: Vec<Schema>,
    /// All snapshots, ordered oldest to newest.
    pub snapshots: Vec<Snapshot>,
    /// Current snapshot ID, if any.
    pub current_snapshot_id: Option<i64>,
    /// Partition spec.
    pub partition_spec: PartitionSpec,
    /// Sort order, if declared.
    pub sort_order: Option<SortOrder>,
    /// All data files referenced by the current snapshot.
    pub data_files: Vec<DataFile>,
    /// Delete files (position or equality deletes) in the current snapshot.
    pub delete_files: Vec<DeleteFile>,
    /// All file paths found in the table's data directory (for orphan detection).
    pub all_storage_paths: Vec<String>,
    /// Total size of metadata files (snapshot JSON + manifest lists + manifests) in bytes.
    pub metadata_size_bytes: u64,
    /// Timestamp when this metadata was collected.
    pub collected_at: DateTime<Utc>,
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Snapshot {
    pub snapshot_id: i64,
    pub timestamp_ms: i64,
    pub summary: HashMap<String, String>,
    pub manifest_list: String,
}

impl Snapshot {
    pub fn timestamp(&self) -> DateTime<Utc> {
        DateTime::from_timestamp_millis(self.timestamp_ms).unwrap_or_default()
    }
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DataFile {
    pub file_path: String,
    pub file_size_bytes: u64,
    pub record_count: u64,
    /// Partition values as key-value pairs.
    pub partition: HashMap<String, String>,
    pub file_format: FileFormat,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeleteFile {
    pub file_path: String,
    pub file_size_bytes: u64,
    pub record_count: u64,
    pub delete_type: DeleteType,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FileFormat {
    Parquet,
    Avro,
    Orc,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeleteType {
    PositionDelete,
    EqualityDelete,
}
