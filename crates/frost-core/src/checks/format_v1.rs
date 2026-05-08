use crate::checks::HealthCheck;
use crate::config::Thresholds;
use crate::metadata::TableMetadata;
use crate::report::{Finding, Severity};
use serde_json::json;

/// Flags tables still on Iceberg format-version 1.
///
/// v1 lacks row-level deletes (no MoR), branching/tagging, equality
/// deletes, and several optimizations that ecosystem readers (Trino,
/// Snowflake, etc.) increasingly assume. Most production Iceberg should
/// be on v2 — flagging v1 as a warning lets teams plan a migration.
pub struct FormatV1Check;

impl HealthCheck for FormatV1Check {
    fn id(&self) -> &'static str {
        "format_v1"
    }

    fn name(&self) -> &'static str {
        "Format Version"
    }

    fn check(&self, metadata: &TableMetadata, _thresholds: &Thresholds) -> Finding {
        if metadata.format_version >= 2 {
            return Finding {
                check_id: self.id().to_string(),
                check_name: self.name().to_string(),
                severity: Severity::Pass,
                message: format!("Format version {} (current spec)", metadata.format_version),
                impact: String::new(),
                fix_suggestion: None,
                fix_command: None,
                estimated_savings: None,
                details: json!({ "format_version": metadata.format_version }),
            };
        }

        Finding {
            check_id: self.id().to_string(),
            check_name: self.name().to_string(),
            severity: Severity::Warning,
            message: "Table is on Iceberg format-version 1".to_string(),
            impact: "v1 lacks row-level deletes (no merge-on-read), equality deletes, \
                     branching/tagging, and several catalog-level features that newer \
                     readers (Trino 4xx+, Snowflake, recent Spark) increasingly assume. \
                     Migration is non-destructive but requires a metadata rewrite."
                .to_string(),
            fix_suggestion: Some(
                "Upgrade the table to format-version 2 by setting the table property \
                 `format-version=2` and rewriting the metadata."
                    .to_string(),
            ),
            fix_command: Some(format!(
                "ALTER TABLE {} SET TBLPROPERTIES ('format-version' = '2')",
                metadata.table_name,
            )),
            estimated_savings: Some(
                "Unlocks merge-on-read, branching/tagging, and equality deletes.".to_string(),
            ),
            details: json!({ "format_version": metadata.format_version }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_helpers::make_test_metadata;

    #[test]
    fn v2_passes() {
        let mut meta = make_test_metadata();
        meta.format_version = 2;
        let f = FormatV1Check.check(&meta, &Thresholds::default());
        assert_eq!(f.severity, Severity::Pass);
    }

    #[test]
    fn v1_warns() {
        let mut meta = make_test_metadata();
        meta.format_version = 1;
        let f = FormatV1Check.check(&meta, &Thresholds::default());
        assert_eq!(f.severity, Severity::Warning);
        assert!(f.fix_command.unwrap().contains("format-version"));
    }
}
