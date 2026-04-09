use crate::checks::HealthCheck;
use crate::config::Thresholds;
use crate::metadata::TableMetadata;
use crate::report::{Finding, Severity};
use chrono::Utc;
use serde_json::json;

pub struct FreshnessCheck;

impl HealthCheck for FreshnessCheck {
    fn id(&self) -> &'static str {
        "freshness"
    }

    fn name(&self) -> &'static str {
        "Table Freshness"
    }

    fn check(&self, metadata: &TableMetadata, thresholds: &Thresholds) -> Finding {
        let latest_snapshot = match metadata.snapshots.last() {
            Some(s) => s,
            None => {
                return Finding {
                    check_id: self.id().to_string(),
                    check_name: self.name().to_string(),
                    severity: Severity::Warning,
                    message: "No snapshots — table has never been written to".to_string(),
                    impact: "Table exists but contains no data.".to_string(),
                    fix_suggestion: None,
                    fix_command: None,
                    estimated_savings: None,
                    details: json!({ "has_snapshots": false }),
                };
            }
        };

        let last_commit = latest_snapshot.timestamp();
        let now = Utc::now();
        let hours_since = (now - last_commit).num_hours();

        if hours_since <= thresholds.stale_table_hours as i64 {
            return Finding {
                check_id: self.id().to_string(),
                check_name: self.name().to_string(),
                severity: Severity::Pass,
                message: format!(
                    "Last commit {} hours ago (threshold: {} hours)",
                    hours_since, thresholds.stale_table_hours,
                ),
                impact: String::new(),
                fix_suggestion: None,
                fix_command: None,
                estimated_savings: None,
                details: json!({
                    "last_commit": last_commit.to_rfc3339(),
                    "hours_since_commit": hours_since,
                    "threshold_hours": thresholds.stale_table_hours,
                }),
            };
        }

        let days_since = hours_since / 24;
        let severity = if hours_since > thresholds.stale_table_hours as i64 * 7 {
            Severity::Critical
        } else {
            Severity::Warning
        };

        Finding {
            check_id: self.id().to_string(),
            check_name: self.name().to_string(),
            severity,
            message: format!(
                "Table is stale — last commit {} days ago ({} hours)",
                days_since, hours_since,
            ),
            impact: "Stale tables may indicate a broken pipeline, failed scheduler, \
                     or decommissioned data source."
                .to_string(),
            fix_suggestion: Some(
                "Investigate the upstream pipeline that writes to this table".to_string(),
            ),
            fix_command: None,
            estimated_savings: None,
            details: json!({
                "last_commit": last_commit.to_rfc3339(),
                "hours_since_commit": hours_since,
                "days_since_commit": days_since,
                "threshold_hours": thresholds.stale_table_hours,
            }),
        }
    }
}
