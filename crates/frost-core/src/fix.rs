//! Fix command generation.
//!
//! Given a finding ID and table metadata, generate the exact command to resolve the issue.

use crate::metadata::TableMetadata;
use serde::{Deserialize, Serialize};

/// A generated fix command with context.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FixCommand {
    pub finding_id: String,
    pub table_name: String,
    /// The executable command (typically Spark SQL CALL statement).
    pub command: String,
    /// What this command does, in plain English.
    pub description: String,
    /// Warnings or prerequisites.
    pub warnings: Vec<String>,
}

/// Generate a fix command for a specific finding.
pub fn generate_fix(table: &TableMetadata, finding_id: &str) -> Option<FixCommand> {
    match finding_id {
        "small_files" => Some(FixCommand {
            finding_id: finding_id.to_string(),
            table_name: table.table_name.clone(),
            command: format!(
                "CALL catalog.system.rewrite_data_files(table => '{}', strategy => 'binpack')",
                table.table_name,
            ),
            description: "Compact small files using bin-pack strategy. Merges small files into \
                         larger ones (target ~256MB per file) to reduce query planning overhead."
                .to_string(),
            warnings: vec![
                "This is a rewrite operation that will create new snapshots.".to_string(),
                "Ensure no concurrent writes to avoid conflicts.".to_string(),
            ],
        }),
        "snapshot_bloat" => Some(FixCommand {
            finding_id: finding_id.to_string(),
            table_name: table.table_name.clone(),
            command: format!(
                "CALL catalog.system.expire_snapshots(table => '{}', older_than => TIMESTAMP '{}')",
                table.table_name,
                (chrono::Utc::now() - chrono::Duration::days(7)).format("%Y-%m-%d %H:%M:%S"),
            ),
            description: "Expire snapshots older than 7 days. This removes snapshot metadata and \
                         allows unreferenced data files to be cleaned up."
                .to_string(),
            warnings: vec![
                "Time-travel queries to expired snapshots will no longer work.".to_string(),
                "Run remove_orphan_files afterward to reclaim storage.".to_string(),
            ],
        }),
        "orphan_files" => Some(FixCommand {
            finding_id: finding_id.to_string(),
            table_name: table.table_name.clone(),
            command: format!(
                "CALL catalog.system.remove_orphan_files(table => '{}')",
                table.table_name,
            ),
            description: "Remove files in the table's data directory that are not referenced by \
                         any snapshot. Reclaims wasted S3 storage."
                .to_string(),
            warnings: vec![
                "Ensure no in-progress writes exist — files from incomplete commits \
                 may be incorrectly identified as orphans."
                    .to_string(),
                "Consider using a conservative older_than parameter (e.g., 3 days) \
                 to avoid deleting files from recent failed writes."
                    .to_string(),
            ],
        }),
        "delete_pressure" => Some(FixCommand {
            finding_id: finding_id.to_string(),
            table_name: table.table_name.clone(),
            command: format!(
                "CALL catalog.system.rewrite_data_files(table => '{}', strategy => 'binpack')",
                table.table_name,
            ),
            description: "Rewrite data files to apply pending deletes. This merges delete files \
                         into data files, eliminating merge-on-read overhead."
                .to_string(),
            warnings: vec![
                "This is a rewrite operation that creates new data files and snapshots.".to_string(),
            ],
        }),
        "metadata_size" => Some(FixCommand {
            finding_id: finding_id.to_string(),
            table_name: table.table_name.clone(),
            command: format!(
                "CALL catalog.system.rewrite_manifests(table => '{}')",
                table.table_name,
            ),
            description: "Rewrite manifest files to optimize metadata size. Combines small \
                         manifests and removes deleted entries."
                .to_string(),
            warnings: vec![
                "Also consider expiring old snapshots to further reduce metadata.".to_string(),
            ],
        }),
        "partition_skew" => Some(FixCommand {
            finding_id: finding_id.to_string(),
            table_name: table.table_name.clone(),
            command: format!(
                "CALL catalog.system.rewrite_data_files(table => '{}', strategy => 'sort')",
                table.table_name,
            ),
            description: "Rewrite data files with sort strategy to rebalance partition sizes."
                .to_string(),
            warnings: vec![
                "Consider if repartitioning (changing partition spec) would be more appropriate \
                 for persistent skew patterns."
                    .to_string(),
            ],
        }),
        _ => None,
    }
}
