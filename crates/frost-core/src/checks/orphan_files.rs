use crate::checks::HealthCheck;
use crate::config::Thresholds;
use crate::metadata::TableMetadata;
use crate::report::{Finding, Severity};
use serde_json::json;
use std::collections::HashSet;

/// Detects unreferenced files in the table's data directory.
///
/// Note on the metadata-only design: we don't have per-file mtimes from a
/// pure-metadata read, so we cannot do the standard "ignore files younger
/// than N days" filter that Spark `remove_orphan_files` does. We compensate
/// by reporting orphan paths sorted, with sample, so the operator can
/// inspect them — and by warning loudly that any fix should pass an
/// `older_than` argument.
pub struct OrphanFilesCheck;

impl HealthCheck for OrphanFilesCheck {
    fn id(&self) -> &'static str {
        "orphan_files"
    }

    fn name(&self) -> &'static str {
        "Orphan Files"
    }

    fn check(&self, metadata: &TableMetadata, _thresholds: &Thresholds) -> Finding {
        // Build set of all referenced file paths (data files + delete files).
        let referenced: HashSet<&str> = metadata
            .data_files
            .iter()
            .map(|f| f.file_path.as_str())
            .chain(metadata.delete_files.iter().map(|f| f.file_path.as_str()))
            .collect();

        let orphans: Vec<&str> = metadata
            .all_storage_paths
            .iter()
            .map(|p| p.as_str())
            .filter(|p| !referenced.contains(p))
            .collect();

        let orphan_count = orphans.len();

        if orphan_count == 0 {
            return Finding {
                check_id: self.id().to_string(),
                check_name: self.name().to_string(),
                severity: Severity::Pass,
                message: "No orphan files detected".to_string(),
                impact: String::new(),
                fix_suggestion: None,
                fix_command: None,
                estimated_savings: None,
                details: json!({ "orphan_count": 0 }),
            };
        }

        let severity = if orphan_count > 100 {
            Severity::Critical
        } else {
            Severity::Warning
        };

        Finding {
            check_id: self.id().to_string(),
            check_name: self.name().to_string(),
            severity,
            message: format!(
                "{} files in data directory not referenced by any snapshot",
                orphan_count,
            ),
            impact: "Orphan files consume S3 storage you're paying for but serve no purpose. \
                     They typically result from failed writes or incomplete compaction. \
                     Files from in-flight commits can falsely look orphaned — always pass \
                     an `older_than` argument to the fix."
                .to_string(),
            fix_suggestion: Some(
                "Run remove_orphan_files with `older_than` set to at least 3 days to \
                 avoid deleting files from incomplete commits."
                    .to_string(),
            ),
            fix_command: Some(format!(
                "CALL catalog.system.remove_orphan_files(table => '{}', older_than => TIMESTAMP '{}')",
                metadata.table_name,
                (chrono::Utc::now() - chrono::Duration::days(3)).format("%Y-%m-%d %H:%M:%S"),
            )),
            estimated_savings: None, // Would need file sizes from storage listing
            details: json!({
                "orphan_count": orphan_count,
                "sample_orphans": orphans.iter().take(10).collect::<Vec<_>>(),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::metadata::*;
    use crate::test_helpers::make_test_metadata;

    #[test]
    fn no_orphans_passes() {
        let mut meta = make_test_metadata();
        meta.data_files = vec![DataFile {
            file_path: "s3://bucket/data/part-0.parquet".to_string(),
            file_size_bytes: 100 * 1024 * 1024,
            record_count: 1_000_000,
            partition: Default::default(),
            file_format: FileFormat::Parquet,
            ..Default::default()
        }];
        meta.all_storage_paths = vec!["s3://bucket/data/part-0.parquet".to_string()];

        let finding = OrphanFilesCheck.check(&meta, &Thresholds::default());
        assert_eq!(finding.severity, Severity::Pass);
    }

    #[test]
    fn detects_orphans() {
        let mut meta = make_test_metadata();
        meta.data_files = vec![DataFile {
            file_path: "s3://bucket/data/part-0.parquet".to_string(),
            file_size_bytes: 100 * 1024 * 1024,
            record_count: 1_000_000,
            partition: Default::default(),
            file_format: FileFormat::Parquet,
            ..Default::default()
        }];
        meta.all_storage_paths = vec![
            "s3://bucket/data/part-0.parquet".to_string(),
            "s3://bucket/data/orphan-1.parquet".to_string(),
            "s3://bucket/data/orphan-2.parquet".to_string(),
        ];

        let finding = OrphanFilesCheck.check(&meta, &Thresholds::default());
        assert_eq!(finding.severity, Severity::Warning);
        assert!(finding.message.contains("2 files"));
        // The fix command must include `older_than` to avoid deleting in-flight files.
        let cmd = finding.fix_command.unwrap();
        assert!(cmd.contains("older_than"));
    }
}
