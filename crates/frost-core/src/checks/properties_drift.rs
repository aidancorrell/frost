use crate::checks::HealthCheck;
use crate::config::Thresholds;
use crate::metadata::TableMetadata;
use crate::report::{Finding, Severity};
use serde_json::json;

/// Detects mismatches between declared table properties and observed
/// reality.
///
/// Today this primarily checks `write.target-file-size-bytes`: if a table
/// declares a 512 MB target but the median data file is 12 MB, that's a
/// strong signal that the writer ignored the property (engine-version
/// mismatch, missing config plumb-through, or wrong commit path).
pub struct PropertiesDriftCheck;

const TARGET_SIZE_KEY: &str = "write.target-file-size-bytes";

impl HealthCheck for PropertiesDriftCheck {
    fn id(&self) -> &'static str {
        "properties_drift"
    }

    fn name(&self) -> &'static str {
        "Properties Drift"
    }

    fn check(&self, metadata: &TableMetadata, thresholds: &Thresholds) -> Finding {
        let declared = metadata
            .properties
            .get(TARGET_SIZE_KEY)
            .and_then(|s| s.parse::<u64>().ok());

        let mut sizes: Vec<u64> = metadata
            .data_files
            .iter()
            .map(|f| f.file_size_bytes)
            .collect();
        sizes.sort_unstable();
        let median_size = if sizes.is_empty() {
            0
        } else {
            sizes[sizes.len() / 2]
        };

        let declared_size = match declared {
            Some(s) if s > 0 => s,
            _ => {
                return Finding {
                    check_id: self.id().to_string(),
                    check_name: self.name().to_string(),
                    severity: Severity::Pass,
                    message: format!(
                        "No `{}` property declared — nothing to compare against",
                        TARGET_SIZE_KEY
                    ),
                    impact: String::new(),
                    fix_suggestion: None,
                    fix_command: None,
                    estimated_savings: None,
                    details: json!({
                        "declared_target": null,
                        "median_file_size": median_size,
                    }),
                };
            }
        };

        if median_size == 0 {
            return Finding {
                check_id: self.id().to_string(),
                check_name: self.name().to_string(),
                severity: Severity::Pass,
                message: "No data files yet — properties drift not assessable".to_string(),
                impact: String::new(),
                fix_suggestion: None,
                fix_command: None,
                estimated_savings: None,
                details: json!({
                    "declared_target": declared_size,
                    "median_file_size": 0,
                }),
            };
        }

        let drift = (median_size as f64 - declared_size as f64).abs() / declared_size as f64;

        if drift <= thresholds.target_file_size_drift_pct {
            return Finding {
                check_id: self.id().to_string(),
                check_name: self.name().to_string(),
                severity: Severity::Pass,
                message: format!(
                    "Median file size {} aligns with declared target {} ({:.0}% drift)",
                    fmt_bytes(median_size),
                    fmt_bytes(declared_size),
                    drift * 100.0,
                ),
                impact: String::new(),
                fix_suggestion: None,
                fix_command: None,
                estimated_savings: None,
                details: json!({
                    "declared_target": declared_size,
                    "median_file_size": median_size,
                    "drift_pct": format!("{:.2}", drift * 100.0),
                }),
            };
        }

        let severity = if drift > 5.0 {
            Severity::Critical
        } else {
            Severity::Warning
        };

        Finding {
            check_id: self.id().to_string(),
            check_name: self.name().to_string(),
            severity,
            message: format!(
                "Median file size {} drifts {:.0}% from declared target {} ({})",
                fmt_bytes(median_size),
                drift * 100.0,
                fmt_bytes(declared_size),
                TARGET_SIZE_KEY,
            ),
            impact: "When the writer ignores a declared file-size target, downstream \
                     readers and compactors disagree about what 'small' means. Common \
                     causes: engine version mismatch, missing Spark config plumb-through, \
                     or a writer that doesn't honor table properties on this commit path."
                .to_string(),
            fix_suggestion: Some(
                "Audit the writer's effective config (check Spark/Flink/Trino session \
                 properties). If the property is intentional, run rewrite_data_files \
                 to reshape existing files toward the target."
                    .to_string(),
            ),
            fix_command: Some(format!(
                "CALL catalog.system.rewrite_data_files(table => '{}', strategy => 'binpack', \
                 options => map('target-file-size-bytes', '{}'))",
                metadata.table_name, declared_size,
            )),
            estimated_savings: None,
            details: json!({
                "declared_target": declared_size,
                "median_file_size": median_size,
                "drift_pct": format!("{:.2}", drift * 100.0),
                "threshold_drift_pct": thresholds.target_file_size_drift_pct * 100.0,
            }),
        }
    }
}

fn fmt_bytes(b: u64) -> String {
    const MB: u64 = 1024 * 1024;
    const GB: u64 = 1024 * MB;
    if b >= GB {
        format!("{:.1} GB", b as f64 / GB as f64)
    } else if b >= MB {
        format!("{:.1} MB", b as f64 / MB as f64)
    } else {
        format!("{} B", b)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::metadata::{DataFile, FileFormat};
    use crate::test_helpers::make_test_metadata;

    fn data_file(size: u64) -> DataFile {
        DataFile {
            file_path: "x".into(),
            file_size_bytes: size,
            record_count: 1000,
            file_format: FileFormat::Parquet,
            ..Default::default()
        }
    }

    #[test]
    fn no_property_passes() {
        let meta = make_test_metadata();
        let f = PropertiesDriftCheck.check(&meta, &Thresholds::default());
        assert_eq!(f.severity, Severity::Pass);
    }

    #[test]
    fn aligned_size_passes() {
        let mut meta = make_test_metadata();
        meta.properties
            .insert(TARGET_SIZE_KEY.into(), (256 * 1024 * 1024).to_string());
        meta.data_files = (0..5).map(|_| data_file(260 * 1024 * 1024)).collect();
        let f = PropertiesDriftCheck.check(&meta, &Thresholds::default());
        assert_eq!(f.severity, Severity::Pass);
    }

    #[test]
    fn large_drift_warns() {
        let mut meta = make_test_metadata();
        meta.properties
            .insert(TARGET_SIZE_KEY.into(), (512 * 1024 * 1024).to_string());
        meta.data_files = (0..5).map(|_| data_file(8 * 1024 * 1024)).collect();
        let f = PropertiesDriftCheck.check(&meta, &Thresholds::default());
        assert!(f.severity >= Severity::Warning);
    }
}
