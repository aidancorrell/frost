use crate::checks::HealthCheck;
use crate::config::Thresholds;
use crate::metadata::TableMetadata;
use crate::report::{Finding, Severity};
use serde_json::json;

/// Checks whether data files are actually sorted according to the table's
/// declared default sort order.
///
/// Each manifest entry records the `sort_order_id` the writer claimed to
/// honor (Iceberg v2). If a table declares a sort order but most files
/// were written with `sort_order_id = 0` (unsorted) or a different ID,
/// readers won't get the data-skipping benefits the sort order is
/// supposed to provide.
pub struct SortComplianceCheck;

impl HealthCheck for SortComplianceCheck {
    fn id(&self) -> &'static str {
        "sort_compliance"
    }

    fn name(&self) -> &'static str {
        "Sort Compliance"
    }

    fn check(&self, metadata: &TableMetadata, thresholds: &Thresholds) -> Finding {
        let declared = match &metadata.sort_order {
            Some(s) if !s.fields.is_empty() => s,
            _ => {
                return Finding {
                    check_id: self.id().to_string(),
                    check_name: self.name().to_string(),
                    severity: Severity::Pass,
                    message: "No sort order declared — compliance check not applicable".to_string(),
                    impact: String::new(),
                    fix_suggestion: None,
                    fix_command: None,
                    estimated_savings: None,
                    details: json!({ "has_sort_order": false }),
                };
            }
        };

        if metadata.data_files.is_empty() {
            return Finding {
                check_id: self.id().to_string(),
                check_name: self.name().to_string(),
                severity: Severity::Pass,
                message: "No data files yet — compliance not assessable".to_string(),
                impact: String::new(),
                fix_suggestion: None,
                fix_command: None,
                estimated_savings: None,
                details: json!({ "has_sort_order": true, "data_files": 0 }),
            };
        }

        let target_id = declared.order_id;
        let total = metadata.data_files.len();
        let mut compliant = 0usize;
        let mut declared_unknown = 0usize;
        for f in &metadata.data_files {
            match f.sort_order_id {
                Some(id) if id == target_id => compliant += 1,
                None => declared_unknown += 1,
                Some(_) => {}
            }
        }

        // Files where the writer didn't declare any sort_order_id at all
        // are common on older writers — don't penalize harshly.
        let assessable = total - declared_unknown;
        if assessable == 0 {
            return Finding {
                check_id: self.id().to_string(),
                check_name: self.name().to_string(),
                severity: Severity::Warning,
                message: format!(
                    "Sort order #{} declared but no files report sort_order_id (older writer?)",
                    target_id,
                ),
                impact: "Without sort_order_id on manifest entries, frost cannot prove \
                         compliance. Newer writers (Spark 3.5+, Trino, Flink recent) \
                         set this — older writers do not."
                    .to_string(),
                fix_suggestion: Some(
                    "Run rewrite_data_files with strategy=sort to rewrite files under \
                     the declared sort order."
                        .to_string(),
                ),
                fix_command: Some(format!(
                    "CALL catalog.system.rewrite_data_files(table => '{}', strategy => 'sort')",
                    metadata.table_name,
                )),
                estimated_savings: None,
                details: json!({
                    "declared_sort_order_id": target_id,
                    "data_files": total,
                    "files_without_sort_order_id": declared_unknown,
                }),
            };
        }

        let pct = (compliant as f64 / assessable as f64) * 100.0;

        if pct >= thresholds.min_sort_compliance_pct {
            return Finding {
                check_id: self.id().to_string(),
                check_name: self.name().to_string(),
                severity: Severity::Pass,
                message: format!(
                    "{:.1}% of files honor sort order #{} ({} of {} assessable)",
                    pct, target_id, compliant, assessable,
                ),
                impact: String::new(),
                fix_suggestion: None,
                fix_command: None,
                estimated_savings: None,
                details: json!({
                    "declared_sort_order_id": target_id,
                    "compliant_files": compliant,
                    "assessable_files": assessable,
                    "compliance_pct": format!("{:.2}", pct),
                }),
            };
        }

        let severity = if pct < thresholds.min_sort_compliance_pct / 2.0 {
            Severity::Critical
        } else {
            Severity::Warning
        };

        Finding {
            check_id: self.id().to_string(),
            check_name: self.name().to_string(),
            severity,
            message: format!(
                "Only {:.1}% of files honor declared sort order #{} ({} of {} assessable, threshold: {:.0}%)",
                pct, target_id, compliant, assessable, thresholds.min_sort_compliance_pct,
            ),
            impact: "When data isn't sorted as declared, readers can't prune by min/max \
                     bounds and queries scan more files than they should. Common cause: \
                     a streaming or backfill job that ignored the sort order."
                .to_string(),
            fix_suggestion: Some(
                "Run rewrite_data_files with strategy=sort to rewrite non-compliant \
                 files under the table's declared sort order."
                    .to_string(),
            ),
            fix_command: Some(format!(
                "CALL catalog.system.rewrite_data_files(table => '{}', strategy => 'sort')",
                metadata.table_name,
            )),
            estimated_savings: Some(
                "Restored data-skipping reduces files scanned per query.".to_string(),
            ),
            details: json!({
                "declared_sort_order_id": target_id,
                "compliant_files": compliant,
                "assessable_files": assessable,
                "files_without_sort_order_id": declared_unknown,
                "compliance_pct": format!("{:.2}", pct),
                "threshold_pct": thresholds.min_sort_compliance_pct,
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::metadata::{DataFile, FileFormat, SortField, SortOrder};
    use crate::test_helpers::make_test_metadata;

    fn file_with_sort(sid: Option<i32>) -> DataFile {
        DataFile {
            file_path: "x".into(),
            file_size_bytes: 1024,
            record_count: 100,
            file_format: FileFormat::Parquet,
            sort_order_id: sid,
            ..Default::default()
        }
    }

    fn meta_with_sort() -> TableMetadata {
        let mut meta = make_test_metadata();
        meta.sort_order = Some(SortOrder {
            order_id: 1,
            fields: vec![SortField {
                source_id: 1,
                transform: "identity".into(),
                direction: "asc".into(),
                null_order: "nulls-first".into(),
            }],
        });
        meta
    }

    #[test]
    fn no_sort_declared_passes() {
        let meta = make_test_metadata();
        let f = SortComplianceCheck.check(&meta, &Thresholds::default());
        assert_eq!(f.severity, Severity::Pass);
    }

    #[test]
    fn high_compliance_passes() {
        let mut meta = meta_with_sort();
        meta.data_files = (0..10).map(|_| file_with_sort(Some(1))).collect();
        let f = SortComplianceCheck.check(&meta, &Thresholds::default());
        assert_eq!(f.severity, Severity::Pass);
    }

    #[test]
    fn low_compliance_warns() {
        let mut meta = meta_with_sort();
        meta.data_files = (0..10)
            .map(|i| file_with_sort(if i < 2 { Some(1) } else { Some(0) }))
            .collect();
        let f = SortComplianceCheck.check(&meta, &Thresholds::default());
        assert!(f.severity >= Severity::Warning);
    }
}
