//! Health check framework.
//!
//! Each check is a struct implementing the `HealthCheck` trait. The engine
//! runs all enabled checks against a `TableMetadata` and collects findings.

pub mod delete_pressure;
pub mod freshness;
pub mod metadata_size;
pub mod orphan_files;
pub mod partition_skew;
pub mod schema_history;
pub mod small_files;
pub mod snapshot_bloat;
pub mod sort_order;

use crate::config::Thresholds;
use crate::metadata::TableMetadata;
use crate::report::Finding;

/// Trait implemented by every health check.
pub trait HealthCheck: Send + Sync {
    /// Machine-readable check ID (e.g., "small_files").
    fn id(&self) -> &'static str;

    /// Human-readable check name.
    fn name(&self) -> &'static str;

    /// Run the check and produce a finding.
    fn check(&self, metadata: &TableMetadata, thresholds: &Thresholds) -> Finding;
}

/// Returns all built-in health checks.
pub fn all_checks() -> Vec<Box<dyn HealthCheck>> {
    vec![
        Box::new(small_files::SmallFilesCheck),
        Box::new(snapshot_bloat::SnapshotBloatCheck),
        Box::new(orphan_files::OrphanFilesCheck),
        Box::new(partition_skew::PartitionSkewCheck),
        Box::new(delete_pressure::DeletePressureCheck),
        Box::new(schema_history::SchemaHistoryCheck),
        Box::new(metadata_size::MetadataSizeCheck),
        Box::new(sort_order::SortOrderCheck),
        Box::new(freshness::FreshnessCheck),
    ]
}
