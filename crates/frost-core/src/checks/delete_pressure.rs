use crate::checks::HealthCheck;
use crate::config::Thresholds;
use crate::metadata::TableMetadata;
use crate::report::{Finding, Severity};
use serde_json::json;

pub struct DeletePressureCheck;

impl HealthCheck for DeletePressureCheck {
    fn id(&self) -> &'static str {
        "delete_pressure"
    }

    fn name(&self) -> &'static str {
        "Delete File Pressure"
    }

    fn check(&self, metadata: &TableMetadata, thresholds: &Thresholds) -> Finding {
        let delete_count = metadata.delete_files.len() as u64;

        if delete_count == 0 {
            return Finding {
                check_id: self.id().to_string(),
                check_name: self.name().to_string(),
                severity: Severity::Pass,
                message: "No outstanding position or equality deletes".to_string(),
                impact: String::new(),
                fix_suggestion: None,
                fix_command: None,
                estimated_savings: None,
                details: json!({ "delete_file_count": 0 }),
            };
        }

        let position_deletes = metadata
            .delete_files
            .iter()
            .filter(|f| f.delete_type == crate::metadata::DeleteType::PositionDelete)
            .count();
        let equality_deletes = metadata
            .delete_files
            .iter()
            .filter(|f| f.delete_type == crate::metadata::DeleteType::EqualityDelete)
            .count();

        let total_delete_bytes: u64 = metadata
            .delete_files
            .iter()
            .map(|f| f.file_size_bytes)
            .sum();

        let severity = if delete_count > thresholds.max_delete_files * 5 {
            Severity::Critical
        } else if delete_count > thresholds.max_delete_files {
            Severity::Warning
        } else {
            return Finding {
                check_id: self.id().to_string(),
                check_name: self.name().to_string(),
                severity: Severity::Pass,
                message: format!(
                    "{} delete files (within threshold of {})",
                    delete_count, thresholds.max_delete_files,
                ),
                impact: String::new(),
                fix_suggestion: None,
                fix_command: None,
                estimated_savings: None,
                details: json!({
                    "delete_file_count": delete_count,
                    "position_deletes": position_deletes,
                    "equality_deletes": equality_deletes,
                }),
            };
        };

        Finding {
            check_id: self.id().to_string(),
            check_name: self.name().to_string(),
            severity,
            message: format!(
                "{} outstanding delete files ({} position, {} equality)",
                delete_count, position_deletes, equality_deletes,
            ),
            impact: "Delete files force merge-on-read, degrading scan performance. \
                     Each query must reconcile deletes against data files at read time."
                .to_string(),
            fix_suggestion: Some(
                "Run rewrite_data_files to compact data files and apply pending deletes"
                    .to_string(),
            ),
            fix_command: Some(format!(
                "CALL catalog.system.rewrite_data_files(table => '{}', strategy => 'binpack')",
                metadata.table_name,
            )),
            estimated_savings: Some(format!(
                "Eliminate merge-on-read overhead for {} delete files ({:.1} MB)",
                delete_count,
                total_delete_bytes as f64 / (1024.0 * 1024.0),
            )),
            details: json!({
                "delete_file_count": delete_count,
                "position_deletes": position_deletes,
                "equality_deletes": equality_deletes,
                "total_delete_bytes": total_delete_bytes,
                "threshold": thresholds.max_delete_files,
            }),
        }
    }
}
