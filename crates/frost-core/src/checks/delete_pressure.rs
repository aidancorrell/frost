use crate::checks::HealthCheck;
use crate::config::Thresholds;
use crate::metadata::{DeleteType, TableMetadata};
use crate::report::{Finding, Severity};
use serde_json::json;

/// Delete-file pressure, weighted by delete type and rows-affected — not
/// just file count.
///
/// Equality deletes force a full predicate scan against every overlapping
/// data file at read time; position deletes only need a sorted lookup. The
/// equality-weight multiplier reflects that asymmetry. We also report the
/// fraction of table rows shadowed by deletes, because a single equality
/// delete that shadows 10% of the table is much worse than 100 deletes that
/// shadow 0.001%.
pub struct DeletePressureCheck;

/// Equality deletes are roughly 5× more expensive at read time than
/// position deletes (full predicate scan vs sorted lookup). This weight
/// is used to compute an "effective" pressure score that the threshold
/// is applied against.
const EQUALITY_DELETE_WEIGHT: f64 = 5.0;

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

        let mut position_count: u64 = 0;
        let mut equality_count: u64 = 0;
        let mut position_rows: u64 = 0;
        let mut equality_rows: u64 = 0;
        let mut total_delete_bytes: u64 = 0;

        for f in &metadata.delete_files {
            total_delete_bytes += f.file_size_bytes;
            match f.delete_type {
                DeleteType::PositionDelete => {
                    position_count += 1;
                    position_rows += f.record_count;
                }
                DeleteType::EqualityDelete => {
                    equality_count += 1;
                    equality_rows += f.record_count;
                }
            }
        }

        let total_delete_rows = position_rows + equality_rows;
        let table_rows: u64 = metadata.data_files.iter().map(|f| f.record_count).sum();
        let row_shadow_pct = if table_rows > 0 {
            (total_delete_rows as f64 / table_rows as f64) * 100.0
        } else {
            0.0
        };

        // Effective pressure: position files weight 1×, equality files
        // weight EQUALITY_DELETE_WEIGHT× because they cost more at scan time.
        let effective_pressure =
            position_count as f64 + (equality_count as f64 * EQUALITY_DELETE_WEIGHT);
        let threshold = thresholds.max_delete_files as f64;

        let severity = if effective_pressure > threshold * 5.0 || row_shadow_pct > 25.0 {
            Severity::Critical
        } else if effective_pressure > threshold || row_shadow_pct > 5.0 {
            Severity::Warning
        } else {
            return Finding {
                check_id: self.id().to_string(),
                check_name: self.name().to_string(),
                severity: Severity::Pass,
                message: format!(
                    "{} delete files ({} position, {} equality), shadowing {:.2}% of rows",
                    delete_count, position_count, equality_count, row_shadow_pct,
                ),
                impact: String::new(),
                fix_suggestion: None,
                fix_command: None,
                estimated_savings: None,
                details: json!({
                    "delete_file_count": delete_count,
                    "position_deletes": position_count,
                    "equality_deletes": equality_count,
                    "delete_rows_total": total_delete_rows,
                    "table_rows": table_rows,
                    "row_shadow_pct": format!("{:.4}", row_shadow_pct),
                    "effective_pressure": format!("{:.1}", effective_pressure),
                    "equality_weight": EQUALITY_DELETE_WEIGHT,
                }),
            };
        };

        Finding {
            check_id: self.id().to_string(),
            check_name: self.name().to_string(),
            severity,
            message: format!(
                "{} delete files ({} position, {} equality) shadowing {:.2}% of rows; effective pressure {:.0} (threshold {})",
                delete_count,
                position_count,
                equality_count,
                row_shadow_pct,
                effective_pressure,
                threshold,
            ),
            impact: format!(
                "Delete files force merge-on-read at scan time. Equality deletes are \
                 ~{:.0}x more expensive than position deletes since they require a full \
                 predicate scan against every overlapping data file. {:.2}% of table \
                 rows are currently shadowed by pending deletes.",
                EQUALITY_DELETE_WEIGHT, row_shadow_pct,
            ),
            fix_suggestion: Some(
                "Compact data files to apply pending deletes and eliminate merge-on-read \
                 overhead."
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
                "position_deletes": position_count,
                "equality_deletes": equality_count,
                "position_delete_rows": position_rows,
                "equality_delete_rows": equality_rows,
                "delete_rows_total": total_delete_rows,
                "table_rows": table_rows,
                "row_shadow_pct": format!("{:.4}", row_shadow_pct),
                "total_delete_bytes": total_delete_bytes,
                "effective_pressure": format!("{:.1}", effective_pressure),
                "equality_weight": EQUALITY_DELETE_WEIGHT,
                "threshold": thresholds.max_delete_files,
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::metadata::{DataFile, DeleteFile, FileFormat};
    use crate::test_helpers::make_test_metadata;

    fn make_data_file(rows: u64) -> DataFile {
        DataFile {
            file_path: "s3://bucket/data/x.parquet".to_string(),
            file_size_bytes: 100 * 1024 * 1024,
            record_count: rows,
            file_format: FileFormat::Parquet,
            ..Default::default()
        }
    }

    #[test]
    fn no_deletes_passes() {
        let meta = make_test_metadata();
        let f = DeletePressureCheck.check(&meta, &Thresholds::default());
        assert_eq!(f.severity, Severity::Pass);
    }

    #[test]
    fn equality_deletes_weighted_higher() {
        let mut meta = make_test_metadata();
        meta.data_files = vec![make_data_file(10_000_000)];
        // 20 equality deletes (effective pressure = 100, above default threshold of 50)
        meta.delete_files = (0..20)
            .map(|i| DeleteFile {
                file_path: format!("s3://bucket/deletes/{i}.parquet"),
                file_size_bytes: 1024,
                record_count: 100,
                delete_type: DeleteType::EqualityDelete,
                equality_ids: vec![1],
            })
            .collect();

        let f = DeletePressureCheck.check(&meta, &Thresholds::default());
        assert!(f.severity >= Severity::Warning);
        assert!(f.message.contains("equality"));
    }

    #[test]
    fn high_row_shadow_is_critical() {
        let mut meta = make_test_metadata();
        meta.data_files = vec![make_data_file(1_000)];
        // Tiny absolute count, but shadows >25% of rows -> Critical.
        meta.delete_files = vec![DeleteFile {
            file_path: "s3://bucket/deletes/big.parquet".to_string(),
            file_size_bytes: 1024,
            record_count: 500,
            delete_type: DeleteType::PositionDelete,
            equality_ids: vec![],
        }];

        let f = DeletePressureCheck.check(&meta, &Thresholds::default());
        assert_eq!(f.severity, Severity::Critical);
    }
}
