use crate::checks::HealthCheck;
use crate::config::Thresholds;
use crate::metadata::TableMetadata;
use crate::report::{Finding, Severity};
use chrono::Utc;
use serde_json::json;

pub struct SnapshotBloatCheck;

impl HealthCheck for SnapshotBloatCheck {
    fn id(&self) -> &'static str {
        "snapshot_bloat"
    }

    fn name(&self) -> &'static str {
        "Snapshot Bloat"
    }

    fn check(&self, metadata: &TableMetadata, thresholds: &Thresholds) -> Finding {
        let snapshot_count = metadata.snapshots.len() as u64;

        if snapshot_count == 0 {
            return Finding {
                check_id: self.id().to_string(),
                check_name: self.name().to_string(),
                severity: Severity::Pass,
                message: "No snapshots (new or empty table)".to_string(),
                impact: String::new(),
                fix_suggestion: None,
                fix_command: None,
                estimated_savings: None,
                details: json!({ "snapshot_count": 0 }),
            };
        }

        let oldest = metadata
            .snapshots
            .iter()
            .map(|s| s.timestamp())
            .min()
            .unwrap();
        let newest = metadata
            .snapshots
            .iter()
            .map(|s| s.timestamp())
            .max()
            .unwrap();

        let now = Utc::now();
        let oldest_age_days = (now - oldest).num_days();

        let count_exceeded = snapshot_count > thresholds.max_snapshots;
        let age_exceeded = oldest_age_days > thresholds.max_snapshot_age_days as i64;

        if !count_exceeded && !age_exceeded {
            return Finding {
                check_id: self.id().to_string(),
                check_name: self.name().to_string(),
                severity: Severity::Pass,
                message: format!(
                    "{} snapshots, oldest: {} days ago",
                    snapshot_count, oldest_age_days
                ),
                impact: String::new(),
                fix_suggestion: None,
                fix_command: None,
                estimated_savings: None,
                details: json!({
                    "snapshot_count": snapshot_count,
                    "oldest_age_days": oldest_age_days,
                }),
            };
        }

        // Estimate metadata bloat: rough heuristic of ~4KB per snapshot for the
        // metadata JSON entry, plus manifest list references.
        let estimated_metadata_waste_bytes =
            snapshot_count.saturating_sub(thresholds.max_snapshots) * 4096;
        let estimated_metadata_waste_gb =
            estimated_metadata_waste_bytes as f64 / (1024.0 * 1024.0 * 1024.0);

        let severity = if snapshot_count > thresholds.max_snapshots * 5 || oldest_age_days > 365 {
            Severity::Critical
        } else {
            Severity::Warning
        };

        let retention_days = thresholds.max_snapshot_age_days;

        Finding {
            check_id: self.id().to_string(),
            check_name: self.name().to_string(),
            severity,
            message: format!(
                "{} snapshots, oldest: {} days ago",
                snapshot_count, oldest_age_days,
            ),
            impact: format!(
                "Metadata grows linearly with snapshot count. {} snapshots means slow \
                 table loading and wasted S3 storage on retained data files.",
                snapshot_count,
            ),
            fix_suggestion: Some(format!(
                "Expire snapshots older than {} days",
                retention_days,
            )),
            fix_command: Some(format!(
                "CALL catalog.system.expire_snapshots(table => '{}', older_than => TIMESTAMP '{}', retain_last => 1)",
                metadata.table_name,
                (now - chrono::Duration::days(retention_days as i64)).format("%Y-%m-%d %H:%M:%S"),
            )),
            estimated_savings: Some(format!(
                "~{:.1} GB metadata reduction, faster table loading",
                estimated_metadata_waste_gb,
            )),
            details: json!({
                "snapshot_count": snapshot_count,
                "oldest_snapshot": oldest.to_rfc3339(),
                "newest_snapshot": newest.to_rfc3339(),
                "oldest_age_days": oldest_age_days,
                "max_snapshots_threshold": thresholds.max_snapshots,
                "max_age_days_threshold": thresholds.max_snapshot_age_days,
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_helpers::make_test_metadata;

    #[test]
    fn few_recent_snapshots_pass() {
        let mut meta = make_test_metadata();
        let now = Utc::now();
        meta.snapshots = (0..5)
            .map(|i| crate::metadata::Snapshot {
                snapshot_id: i,
                timestamp_ms: (now - chrono::Duration::hours(i)).timestamp_millis(),
                summary: Default::default(),
                manifest_list: format!("s3://bucket/metadata/snap-{i}-manifest-list.avro"),
                ..Default::default()
            })
            .collect();

        let finding = SnapshotBloatCheck.check(&meta, &Thresholds::default());
        assert_eq!(finding.severity, Severity::Pass);
    }

    #[test]
    fn many_old_snapshots_warn() {
        let mut meta = make_test_metadata();
        let now = Utc::now();
        meta.snapshots = (0..150)
            .map(|i| crate::metadata::Snapshot {
                snapshot_id: i,
                timestamp_ms: (now - chrono::Duration::days(i)).timestamp_millis(),
                summary: Default::default(),
                manifest_list: format!("s3://bucket/metadata/snap-{i}-manifest-list.avro"),
                ..Default::default()
            })
            .collect();

        let finding = SnapshotBloatCheck.check(&meta, &Thresholds::default());
        assert!(finding.severity >= Severity::Warning);
    }
}
