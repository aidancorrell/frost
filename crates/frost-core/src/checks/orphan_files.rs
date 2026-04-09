use crate::checks::HealthCheck;
use crate::config::Thresholds;
use crate::metadata::TableMetadata;
use crate::report::{Finding, Severity};
use serde_json::json;
use std::collections::HashSet;

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
                     They typically result from failed writes or incomplete compaction."
                .to_string(),
            fix_suggestion: Some(
                "Run remove_orphan_files to clean up unreferenced data".to_string(),
            ),
            fix_command: Some(format!(
                "CALL catalog.system.remove_orphan_files(table => '{}')",
                metadata.table_name,
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
        }];
        meta.all_storage_paths = vec![
            "s3://bucket/data/part-0.parquet".to_string(),
            "s3://bucket/data/orphan-1.parquet".to_string(),
            "s3://bucket/data/orphan-2.parquet".to_string(),
        ];

        let finding = OrphanFilesCheck.check(&meta, &Thresholds::default());
        assert_eq!(finding.severity, Severity::Warning);
        assert!(finding.message.contains("2 files"));
    }
}
