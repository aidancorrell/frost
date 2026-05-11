//! Fix command generation.
//!
//! Given a finding ID and table metadata, generate the exact command to resolve the issue.
//! Each fix carries an estimated scope so callers (especially agents) can
//! reason about the cost of running the fix.

use crate::metadata::{DeleteType, TableMetadata};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

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
    /// Estimated scope of this fix — what it will actually rewrite/expire.
    /// Used by `dry_run_fix` and surfaced in the `get_fix` response.
    pub scope: FixScope,
}

/// Scope of a fix operation, computed from the current table metadata.
/// Treat numbers as upper-bound estimates: the actual fix may touch fewer
/// objects (e.g., compaction may skip files already at target size).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FixScope {
    /// Approximate number of files the fix will read or rewrite.
    pub estimated_files: u64,
    /// Approximate bytes the fix will touch (read + write combined).
    pub estimated_bytes: u64,
    /// Number of distinct partitions involved.
    pub estimated_partitions: u64,
    /// Number of snapshots the fix will expire (snapshot_bloat only).
    pub estimated_snapshots_expired: u64,
    /// Free-form summary of why the scope is what it is.
    pub summary: String,
}

/// Generate a fix command for a specific finding.
pub fn generate_fix(table: &TableMetadata, finding_id: &str) -> Option<FixCommand> {
    match finding_id {
        "small_files" => Some(small_files_fix(table)),
        "snapshot_bloat" => Some(snapshot_bloat_fix(table)),
        "orphan_files" => Some(orphan_files_fix(table)),
        "delete_pressure" => Some(delete_pressure_fix(table)),
        "metadata_size" => Some(metadata_size_fix(table)),
        "partition_skew" => Some(partition_skew_fix(table)),
        "format_v1" => Some(format_v1_fix(table)),
        "properties_drift" => Some(properties_drift_fix(table)),
        "partition_spec_evolution" => Some(spec_evolution_fix(table)),
        "sort_compliance" => Some(sort_compliance_fix(table)),
        "stats_coverage" => Some(stats_coverage_fix(table)),
        _ => None,
    }
}

fn distinct_partitions(table: &TableMetadata) -> u64 {
    let mut keys: HashSet<String> = HashSet::new();
    for f in &table.data_files {
        let mut parts: Vec<_> = f.partition.iter().collect();
        parts.sort_by_key(|(k, _)| (*k).clone());
        keys.insert(
            parts
                .iter()
                .map(|(k, v)| format!("{k}={v}"))
                .collect::<Vec<_>>()
                .join("/"),
        );
    }
    keys.len() as u64
}

fn small_files_fix(table: &TableMetadata) -> FixCommand {
    let small_threshold = 8 * 1024 * 1024;
    let small: Vec<_> = table
        .data_files
        .iter()
        .filter(|f| f.file_size_bytes < small_threshold)
        .collect();
    let bytes: u64 = small.iter().map(|f| f.file_size_bytes).sum();
    let parts: HashSet<String> = small
        .iter()
        .map(|f| {
            let mut p: Vec<_> = f.partition.iter().collect();
            p.sort_by_key(|(k, _)| (*k).clone());
            p.iter()
                .map(|(k, v)| format!("{k}={v}"))
                .collect::<Vec<_>>()
                .join("/")
        })
        .collect();

    FixCommand {
        finding_id: "small_files".into(),
        table_name: table.table_name.clone(),
        command: format!(
            "CALL catalog.system.rewrite_data_files(table => '{}', strategy => 'binpack')",
            table.table_name,
        ),
        description: "Compact small files using bin-pack strategy. Merges small files into \
                     larger ones (target ~256 MB) to reduce query planning overhead."
            .to_string(),
        warnings: vec![
            "This is a rewrite operation that will create new snapshots.".to_string(),
            "Ensure no concurrent writes to avoid conflicts.".to_string(),
        ],
        scope: FixScope {
            estimated_files: small.len() as u64,
            estimated_bytes: bytes,
            estimated_partitions: parts.len() as u64,
            estimated_snapshots_expired: 0,
            summary: format!(
                "Rewrites ~{} small files ({:.1} MB) across {} partition(s)",
                small.len(),
                bytes as f64 / (1024.0 * 1024.0),
                parts.len(),
            ),
        },
    }
}

fn snapshot_bloat_fix(table: &TableMetadata) -> FixCommand {
    let cutoff_days = 7;
    let cutoff = chrono::Utc::now() - chrono::Duration::days(cutoff_days);
    let to_expire = table
        .snapshots
        .iter()
        .filter(|s| s.timestamp() < cutoff)
        .count() as u64;

    FixCommand {
        finding_id: "snapshot_bloat".into(),
        table_name: table.table_name.clone(),
        command: format!(
            "CALL catalog.system.expire_snapshots(table => '{}', older_than => TIMESTAMP '{}')",
            table.table_name,
            cutoff.format("%Y-%m-%d %H:%M:%S"),
        ),
        description: format!(
            "Expire snapshots older than {} days. Removes snapshot metadata and allows \
             unreferenced data files to be cleaned up.",
            cutoff_days
        ),
        warnings: vec![
            "Time-travel queries to expired snapshots will no longer work.".to_string(),
            "Run remove_orphan_files afterward to reclaim storage.".to_string(),
        ],
        scope: FixScope {
            estimated_files: 0,
            estimated_bytes: 0,
            estimated_partitions: 0,
            estimated_snapshots_expired: to_expire,
            summary: format!("Expires ~{} snapshot(s)", to_expire),
        },
    }
}

fn orphan_files_fix(table: &TableMetadata) -> FixCommand {
    let referenced: HashSet<&str> = table
        .data_files
        .iter()
        .map(|f| f.file_path.as_str())
        .chain(table.delete_files.iter().map(|f| f.file_path.as_str()))
        .collect();
    let orphans: u64 = table
        .all_storage_paths
        .iter()
        .filter(|p| !referenced.contains(p.as_str()))
        .count() as u64;

    FixCommand {
        finding_id: "orphan_files".into(),
        table_name: table.table_name.clone(),
        command: format!(
            "CALL catalog.system.remove_orphan_files(table => '{}', older_than => TIMESTAMP '{}')",
            table.table_name,
            (chrono::Utc::now() - chrono::Duration::days(3)).format("%Y-%m-%d %H:%M:%S"),
        ),
        description: "Remove files in the table's data directory that are not referenced by \
                     any snapshot. Reclaims wasted S3 storage."
            .to_string(),
        warnings: vec![
            "Ensure no in-progress writes exist — files from incomplete commits \
             may be incorrectly identified as orphans."
                .to_string(),
            "The 3-day `older_than` argument protects against deleting in-flight commit files."
                .to_string(),
        ],
        scope: FixScope {
            estimated_files: orphans,
            estimated_bytes: 0,
            estimated_partitions: 0,
            estimated_snapshots_expired: 0,
            summary: format!("Removes ~{} orphan files", orphans),
        },
    }
}

fn delete_pressure_fix(table: &TableMetadata) -> FixCommand {
    let position = table
        .delete_files
        .iter()
        .filter(|f| f.delete_type == DeleteType::PositionDelete)
        .count();
    let equality = table
        .delete_files
        .iter()
        .filter(|f| f.delete_type == DeleteType::EqualityDelete)
        .count();
    let total_files: u64 = table.data_files.len() as u64;
    let total_bytes: u64 = table.data_files.iter().map(|f| f.file_size_bytes).sum();
    let parts = distinct_partitions(table);

    FixCommand {
        finding_id: "delete_pressure".into(),
        table_name: table.table_name.clone(),
        command: format!(
            "CALL catalog.system.rewrite_data_files(table => '{}', strategy => 'binpack')",
            table.table_name,
        ),
        description: format!(
            "Rewrite data files to apply pending deletes ({} position, {} equality). \
             Eliminates merge-on-read overhead.",
            position, equality,
        ),
        warnings: vec![
            "This is a rewrite operation that creates new data files and snapshots.".to_string(),
        ],
        scope: FixScope {
            estimated_files: total_files,
            estimated_bytes: total_bytes,
            estimated_partitions: parts,
            estimated_snapshots_expired: 0,
            summary: format!(
                "Rewrites up to {} data files ({:.1} GB) to apply {} delete file(s)",
                total_files,
                total_bytes as f64 / (1024.0 * 1024.0 * 1024.0),
                position + equality,
            ),
        },
    }
}

fn metadata_size_fix(table: &TableMetadata) -> FixCommand {
    FixCommand {
        finding_id: "metadata_size".into(),
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
        scope: FixScope {
            estimated_files: table.manifest_stats.manifest_count,
            estimated_bytes: table.manifest_stats.manifests_total_bytes,
            estimated_partitions: 0,
            estimated_snapshots_expired: 0,
            summary: format!(
                "Rewrites {} manifest file(s) ({:.1} MB)",
                table.manifest_stats.manifest_count,
                table.manifest_stats.manifests_total_bytes as f64 / (1024.0 * 1024.0),
            ),
        },
    }
}

fn partition_skew_fix(table: &TableMetadata) -> FixCommand {
    let total_files = table.data_files.len() as u64;
    let total_bytes: u64 = table.data_files.iter().map(|f| f.file_size_bytes).sum();
    let parts = distinct_partitions(table);
    FixCommand {
        finding_id: "partition_skew".into(),
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
        scope: FixScope {
            estimated_files: total_files,
            estimated_bytes: total_bytes,
            estimated_partitions: parts,
            estimated_snapshots_expired: 0,
            summary: format!(
                "Rewrites up to {} files ({:.1} GB) across {} partitions",
                total_files,
                total_bytes as f64 / (1024.0 * 1024.0 * 1024.0),
                parts,
            ),
        },
    }
}

fn format_v1_fix(table: &TableMetadata) -> FixCommand {
    FixCommand {
        finding_id: "format_v1".into(),
        table_name: table.table_name.clone(),
        command: format!(
            "ALTER TABLE {} SET TBLPROPERTIES ('format-version' = '2')",
            table.table_name,
        ),
        description: "Upgrade Iceberg table from format-version 1 to 2. Non-destructive — \
                     existing data files are unchanged."
            .to_string(),
        warnings: vec![
            "Older readers (Spark <3.3, Trino <380) cannot read v2 tables. Verify all \
             consumers support v2 before upgrading."
                .to_string(),
        ],
        scope: FixScope {
            estimated_files: 0,
            estimated_bytes: 0,
            estimated_partitions: 0,
            estimated_snapshots_expired: 0,
            summary: "Metadata-only upgrade — no data file rewrite".to_string(),
        },
    }
}

fn properties_drift_fix(table: &TableMetadata) -> FixCommand {
    let target = table
        .properties
        .get("write.target-file-size-bytes")
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(536_870_912);

    let total_files = table.data_files.len() as u64;
    let total_bytes: u64 = table.data_files.iter().map(|f| f.file_size_bytes).sum();

    FixCommand {
        finding_id: "properties_drift".into(),
        table_name: table.table_name.clone(),
        command: format!(
            "CALL catalog.system.rewrite_data_files(table => '{}', strategy => 'binpack', \
             options => map('target-file-size-bytes', '{}'))",
            table.table_name, target,
        ),
        description: format!(
            "Rewrite files toward declared target {} bytes. Audit the writer's effective \
             config — if the property was set after data was written, that's expected.",
            target,
        ),
        warnings: vec![
            "If the property was set incorrectly, fix it via ALTER TABLE first.".to_string(),
        ],
        scope: FixScope {
            estimated_files: total_files,
            estimated_bytes: total_bytes,
            estimated_partitions: distinct_partitions(table),
            estimated_snapshots_expired: 0,
            summary: format!(
                "Rewrites up to {} files toward target file size",
                total_files,
            ),
        },
    }
}

fn spec_evolution_fix(table: &TableMetadata) -> FixCommand {
    let default_id = table.partition_spec.spec_id;
    let to_rewrite = table
        .data_files
        .iter()
        .filter(|f| f.spec_id.map(|s| s != default_id).unwrap_or(false))
        .count() as u64;
    let bytes: u64 = table
        .data_files
        .iter()
        .filter(|f| f.spec_id.map(|s| s != default_id).unwrap_or(false))
        .map(|f| f.file_size_bytes)
        .sum();

    FixCommand {
        finding_id: "partition_spec_evolution".into(),
        table_name: table.table_name.clone(),
        command: format!(
            "CALL catalog.system.rewrite_data_files(table => '{}', \
             options => map('rewrite-all', 'true'))",
            table.table_name,
        ),
        description: "Rewrite files under retired partition specs onto the current spec, \
                     allowing planner-time spec resolution to drop them."
            .to_string(),
        warnings: vec!["Full table rewrite — can be expensive for large tables.".to_string()],
        scope: FixScope {
            estimated_files: to_rewrite,
            estimated_bytes: bytes,
            estimated_partitions: 0,
            estimated_snapshots_expired: 0,
            summary: format!(
                "Rewrites {} files ({:.1} GB) currently under retired specs",
                to_rewrite,
                bytes as f64 / (1024.0 * 1024.0 * 1024.0),
            ),
        },
    }
}

fn sort_compliance_fix(table: &TableMetadata) -> FixCommand {
    let target_id = table.sort_order.as_ref().map(|s| s.order_id).unwrap_or(0);
    let non_compliant = table
        .data_files
        .iter()
        .filter(|f| f.sort_order_id.map(|sid| sid != target_id).unwrap_or(false))
        .count() as u64;
    let bytes: u64 = table
        .data_files
        .iter()
        .filter(|f| f.sort_order_id.map(|sid| sid != target_id).unwrap_or(false))
        .map(|f| f.file_size_bytes)
        .sum();

    FixCommand {
        finding_id: "sort_compliance".into(),
        table_name: table.table_name.clone(),
        command: format!(
            "CALL catalog.system.rewrite_data_files(table => '{}', strategy => 'sort')",
            table.table_name,
        ),
        description: "Rewrite non-compliant files under the table's declared sort order. \
                     Restores file pruning by min/max bounds."
            .to_string(),
        warnings: vec![
            "Sort rewrites are more expensive than binpack — schedule off-peak.".to_string(),
        ],
        scope: FixScope {
            estimated_files: non_compliant,
            estimated_bytes: bytes,
            estimated_partitions: distinct_partitions(table),
            estimated_snapshots_expired: 0,
            summary: format!(
                "Rewrites {} non-compliant files ({:.1} GB)",
                non_compliant,
                bytes as f64 / (1024.0 * 1024.0 * 1024.0),
            ),
        },
    }
}

fn stats_coverage_fix(table: &TableMetadata) -> FixCommand {
    let missing = table
        .data_files
        .iter()
        .filter(|f| {
            f.value_counts.is_empty() && f.null_value_counts.is_empty() && f.column_sizes.is_empty()
        })
        .count() as u64;
    let bytes: u64 = table
        .data_files
        .iter()
        .filter(|f| f.value_counts.is_empty() && f.null_value_counts.is_empty())
        .map(|f| f.file_size_bytes)
        .sum();

    FixCommand {
        finding_id: "stats_coverage".into(),
        table_name: table.table_name.clone(),
        command: format!(
            "CALL catalog.system.rewrite_data_files(table => '{}', strategy => 'binpack', \
             options => map('rewrite-all', 'true'))",
            table.table_name,
        ),
        description: "Rewrite files with a recent writer to populate missing column statistics. \
                     Restores planner-time file pruning."
            .to_string(),
        warnings: vec![
            "Use a recent engine version (Spark 3.5+, Trino 4xx+, Flink 1.18+) — older \
             writers do not emit stats consistently."
                .to_string(),
        ],
        scope: FixScope {
            estimated_files: missing,
            estimated_bytes: bytes,
            estimated_partitions: distinct_partitions(table),
            estimated_snapshots_expired: 0,
            summary: format!(
                "Rewrites {} files ({:.1} GB) lacking column statistics",
                missing,
                bytes as f64 / (1024.0 * 1024.0 * 1024.0),
            ),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::metadata::{DataFile, FileFormat};
    use crate::test_helpers::make_test_metadata;

    #[test]
    fn small_files_fix_reports_scope() {
        let mut meta = make_test_metadata();
        meta.data_files = (0..5)
            .map(|i| DataFile {
                file_path: format!("s3://x/{i}.parquet"),
                file_size_bytes: 1024,
                record_count: 1,
                file_format: FileFormat::Parquet,
                ..Default::default()
            })
            .collect();
        let cmd = generate_fix(&meta, "small_files").unwrap();
        assert_eq!(cmd.scope.estimated_files, 5);
        assert!(cmd.scope.estimated_bytes > 0);
        assert!(cmd.scope.summary.contains("5 small files"));
    }

    #[test]
    fn unknown_finding_returns_none() {
        let meta = make_test_metadata();
        assert!(generate_fix(&meta, "nonexistent_check").is_none());
    }

    #[test]
    fn format_v1_fix_is_metadata_only() {
        let meta = make_test_metadata();
        let cmd = generate_fix(&meta, "format_v1").unwrap();
        assert_eq!(cmd.scope.estimated_files, 0);
        assert!(cmd.scope.summary.contains("Metadata-only"));
    }
}
