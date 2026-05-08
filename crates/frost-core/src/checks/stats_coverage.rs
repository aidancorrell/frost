use crate::checks::HealthCheck;
use crate::config::Thresholds;
use crate::metadata::TableMetadata;
use crate::report::{Finding, Severity};
use serde_json::json;

/// Reports the fraction of data files that carry per-column statistics
/// (value counts and null counts).
///
/// Without column stats, the planner cannot prune files by min/max
/// bounds, can't push down `IS NULL` predicates, and can't cost-model
/// joins accurately. Recent writers always emit these — older writers
/// or some streaming paths do not. Low coverage is a real planner-time
/// regression even if every other health metric looks fine.
pub struct StatsCoverageCheck;

impl HealthCheck for StatsCoverageCheck {
    fn id(&self) -> &'static str {
        "stats_coverage"
    }

    fn name(&self) -> &'static str {
        "Statistics Coverage"
    }

    fn check(&self, metadata: &TableMetadata, thresholds: &Thresholds) -> Finding {
        if metadata.data_files.is_empty() {
            return Finding {
                check_id: self.id().to_string(),
                check_name: self.name().to_string(),
                severity: Severity::Pass,
                message: "No data files to assess".to_string(),
                impact: String::new(),
                fix_suggestion: None,
                fix_command: None,
                estimated_savings: None,
                details: json!({ "data_files": 0 }),
            };
        }

        let total = metadata.data_files.len();
        let with_value_counts = metadata
            .data_files
            .iter()
            .filter(|f| !f.value_counts.is_empty())
            .count();
        let with_null_counts = metadata
            .data_files
            .iter()
            .filter(|f| !f.null_value_counts.is_empty())
            .count();
        let with_any_stats = metadata
            .data_files
            .iter()
            .filter(|f| {
                !f.value_counts.is_empty()
                    || !f.null_value_counts.is_empty()
                    || !f.column_sizes.is_empty()
            })
            .count();

        let coverage_pct = (with_any_stats as f64 / total as f64) * 100.0;

        if coverage_pct >= thresholds.min_stats_coverage_pct {
            return Finding {
                check_id: self.id().to_string(),
                check_name: self.name().to_string(),
                severity: Severity::Pass,
                message: format!("{:.1}% of data files carry column statistics", coverage_pct,),
                impact: String::new(),
                fix_suggestion: None,
                fix_command: None,
                estimated_savings: None,
                details: json!({
                    "data_files": total,
                    "files_with_value_counts": with_value_counts,
                    "files_with_null_counts": with_null_counts,
                    "files_with_any_stats": with_any_stats,
                    "coverage_pct": format!("{:.2}", coverage_pct),
                }),
            };
        }

        let severity = if coverage_pct < thresholds.min_stats_coverage_pct / 2.0 {
            Severity::Critical
        } else {
            Severity::Warning
        };

        Finding {
            check_id: self.id().to_string(),
            check_name: self.name().to_string(),
            severity,
            message: format!(
                "Only {:.1}% of files carry column statistics ({} of {}, threshold: {:.0}%)",
                coverage_pct, with_any_stats, total, thresholds.min_stats_coverage_pct,
            ),
            impact: "Without column statistics in manifest entries, the query planner \
                     cannot prune files by min/max bounds or push down null predicates. \
                     Common cause: an older writer (Spark <3.3, some streaming paths) \
                     that emits empty stats maps."
                .to_string(),
            fix_suggestion: Some(
                "Rewrite affected files with a recent writer to populate manifest \
                 column statistics. `rewrite_data_files` with strategy=binpack will \
                 do this as a side effect."
                    .to_string(),
            ),
            fix_command: Some(format!(
                "CALL catalog.system.rewrite_data_files(table => '{}', strategy => 'binpack', \
                 options => map('rewrite-all', 'true'))",
                metadata.table_name,
            )),
            estimated_savings: Some(
                "Restored file pruning reduces scanned files per query.".to_string(),
            ),
            details: json!({
                "data_files": total,
                "files_with_value_counts": with_value_counts,
                "files_with_null_counts": with_null_counts,
                "files_with_any_stats": with_any_stats,
                "coverage_pct": format!("{:.2}", coverage_pct),
                "threshold_pct": thresholds.min_stats_coverage_pct,
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::metadata::{DataFile, FileFormat};
    use crate::test_helpers::make_test_metadata;
    use std::collections::HashMap;

    #[test]
    fn no_files_passes() {
        let meta = make_test_metadata();
        let f = StatsCoverageCheck.check(&meta, &Thresholds::default());
        assert_eq!(f.severity, Severity::Pass);
    }

    #[test]
    fn high_coverage_passes() {
        let mut meta = make_test_metadata();
        let mut vc = HashMap::new();
        vc.insert(1, 100);
        meta.data_files = (0..10)
            .map(|_| DataFile {
                file_path: "x".into(),
                file_size_bytes: 1024,
                record_count: 100,
                file_format: FileFormat::Parquet,
                value_counts: vc.clone(),
                ..Default::default()
            })
            .collect();
        let f = StatsCoverageCheck.check(&meta, &Thresholds::default());
        assert_eq!(f.severity, Severity::Pass);
    }

    #[test]
    fn low_coverage_warns() {
        let mut meta = make_test_metadata();
        meta.data_files = (0..10)
            .map(|_| DataFile {
                file_path: "x".into(),
                file_size_bytes: 1024,
                record_count: 100,
                file_format: FileFormat::Parquet,
                ..Default::default()
            })
            .collect();
        let f = StatsCoverageCheck.check(&meta, &Thresholds::default());
        assert!(f.severity >= Severity::Warning);
    }
}
