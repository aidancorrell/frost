use crate::checks::HealthCheck;
use crate::config::Thresholds;
use crate::metadata::TableMetadata;
use crate::report::{Finding, Severity};
use serde_json::json;

pub struct MetadataSizeCheck;

impl HealthCheck for MetadataSizeCheck {
    fn id(&self) -> &'static str {
        "metadata_size"
    }

    fn name(&self) -> &'static str {
        "Metadata Size"
    }

    fn check(&self, metadata: &TableMetadata, thresholds: &Thresholds) -> Finding {
        let size = metadata.metadata_size_bytes;

        if size <= thresholds.max_metadata_bytes {
            let size_mb = size as f64 / (1024.0 * 1024.0);
            let threshold_mb = thresholds.max_metadata_bytes as f64 / (1024.0 * 1024.0);
            return Finding {
                check_id: self.id().to_string(),
                check_name: self.name().to_string(),
                severity: Severity::Pass,
                message: format!(
                    "Metadata size {:.1} MB (threshold: {:.0} MB)",
                    size_mb, threshold_mb,
                ),
                impact: String::new(),
                fix_suggestion: None,
                fix_command: None,
                estimated_savings: None,
                details: json!({
                    "metadata_size_bytes": size,
                    "threshold_bytes": thresholds.max_metadata_bytes,
                }),
            };
        }

        let size_mb = size as f64 / (1024.0 * 1024.0);
        let severity = if size > thresholds.max_metadata_bytes * 3 {
            Severity::Critical
        } else {
            Severity::Warning
        };

        Finding {
            check_id: self.id().to_string(),
            check_name: self.name().to_string(),
            severity,
            message: format!("Metadata size is {:.1} MB — oversized", size_mb),
            impact: "Large metadata slows table loading in every engine. Spark, Trino, and \
                     other readers must download and parse all metadata before planning queries."
                .to_string(),
            fix_suggestion: Some(
                "Expire old snapshots and rewrite manifests to reduce metadata volume".to_string(),
            ),
            fix_command: Some(format!(
                "CALL catalog.system.rewrite_manifests(table => '{}')",
                metadata.table_name,
            )),
            estimated_savings: Some(format!(
                "~{:.0} MB metadata reduction possible",
                size_mb * 0.5,
            )),
            details: json!({
                "metadata_size_bytes": size,
                "metadata_size_mb": format!("{:.1}", size_mb),
                "threshold_bytes": thresholds.max_metadata_bytes,
            }),
        }
    }
}
