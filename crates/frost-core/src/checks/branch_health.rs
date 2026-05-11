use crate::checks::HealthCheck;
use crate::config::Thresholds;
use crate::metadata::TableMetadata;
use crate::report::{Finding, Severity};
use chrono::{DateTime, Utc};
use serde_json::json;
use std::collections::HashMap;

/// Per-branch and per-tag health.
///
/// Iceberg v2 supports named refs (branches and tags). Each one targets a
/// specific snapshot. A common operational problem is a branch that's
/// pinned to a snapshot from months ago: it prevents snapshot expiration
/// (the snapshot can't be expired while a ref points at it), bloating
/// metadata. We flag stale branches and identify the snapshot they're
/// pinning open.
pub struct BranchHealthCheck;

impl HealthCheck for BranchHealthCheck {
    fn id(&self) -> &'static str {
        "branch_health"
    }

    fn name(&self) -> &'static str {
        "Branch Health"
    }

    fn check(&self, metadata: &TableMetadata, thresholds: &Thresholds) -> Finding {
        // Skip the implicit `main` branch — its staleness is already
        // captured by the `freshness` check.
        let extra_refs: Vec<_> = metadata
            .refs
            .iter()
            .filter(|(name, _)| name.as_str() != "main")
            .collect();

        if extra_refs.is_empty() {
            return Finding {
                check_id: self.id().to_string(),
                check_name: self.name().to_string(),
                severity: Severity::Pass,
                message: "No branches or tags beyond `main`".to_string(),
                impact: String::new(),
                fix_suggestion: None,
                fix_command: None,
                estimated_savings: None,
                details: json!({ "ref_count": 0 }),
            };
        }

        // Build a snapshot_id -> timestamp lookup so we can age each ref.
        let snap_times: HashMap<i64, DateTime<Utc>> = metadata
            .snapshots
            .iter()
            .map(|s| (s.snapshot_id, s.timestamp()))
            .collect();

        let stale_threshold = chrono::Duration::days(thresholds.stale_branch_days as i64);
        let now = Utc::now();

        let mut stale = Vec::new();
        let mut healthy = Vec::new();
        let mut dangling = Vec::new();
        let mut branch_count = 0u64;
        let mut tag_count = 0u64;

        for (name, r) in &extra_refs {
            if r.ref_type == "branch" {
                branch_count += 1;
            } else {
                tag_count += 1;
            }
            match snap_times.get(&r.snapshot_id) {
                Some(ts) => {
                    let age = now - *ts;
                    let age_days = age.num_days();
                    if age > stale_threshold {
                        stale.push(json!({
                            "name": name,
                            "type": r.ref_type,
                            "snapshot_id": r.snapshot_id,
                            "snapshot_age_days": age_days,
                        }));
                    } else {
                        healthy.push(json!({
                            "name": name,
                            "type": r.ref_type,
                            "snapshot_id": r.snapshot_id,
                            "snapshot_age_days": age_days,
                        }));
                    }
                }
                None => {
                    dangling.push(json!({
                        "name": name,
                        "type": r.ref_type,
                        "snapshot_id": r.snapshot_id,
                    }));
                }
            }
        }

        let mut messages = Vec::new();
        if !stale.is_empty() {
            messages.push(format!(
                "{} stale ref(s) older than {} days",
                stale.len(),
                thresholds.stale_branch_days,
            ));
        }
        if !dangling.is_empty() {
            messages.push(format!(
                "{} ref(s) pointing at snapshots no longer in metadata",
                dangling.len(),
            ));
        }

        if stale.is_empty() && dangling.is_empty() {
            return Finding {
                check_id: self.id().to_string(),
                check_name: self.name().to_string(),
                severity: Severity::Pass,
                message: format!(
                    "{} branch(es), {} tag(s) — all healthy",
                    branch_count, tag_count,
                ),
                impact: String::new(),
                fix_suggestion: None,
                fix_command: None,
                estimated_savings: None,
                details: json!({
                    "branches": branch_count,
                    "tags": tag_count,
                    "healthy_refs": healthy,
                }),
            };
        }

        let severity = if !dangling.is_empty() {
            Severity::Critical
        } else {
            Severity::Warning
        };

        Finding {
            check_id: self.id().to_string(),
            check_name: self.name().to_string(),
            severity,
            message: messages.join("; "),
            impact: "Refs pin their target snapshot open — `expire_snapshots` cannot \
                     drop a snapshot that any ref points at. Stale branches and \
                     forgotten tags silently bloat metadata. Dangling refs (pointing \
                     at expired snapshots) indicate metadata corruption."
                .to_string(),
            fix_suggestion: Some(
                "Drop refs that are no longer needed. For audit/release branches, \
                 set `max-ref-age-ms` so they auto-expire."
                    .to_string(),
            ),
            fix_command: Some(format!(
                "ALTER TABLE {} DROP BRANCH `<name>`  -- repeat per stale ref",
                metadata.table_name,
            )),
            estimated_savings: Some(
                "Unblocking snapshot expiration reduces metadata size.".to_string(),
            ),
            details: json!({
                "branches": branch_count,
                "tags": tag_count,
                "stale_refs": stale,
                "dangling_refs": dangling,
                "healthy_refs": healthy,
                "stale_threshold_days": thresholds.stale_branch_days,
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::metadata::{Snapshot, SnapshotRef};
    use crate::test_helpers::make_test_metadata;
    use chrono::Duration;

    #[test]
    fn no_extra_refs_passes() {
        let meta = make_test_metadata();
        let f = BranchHealthCheck.check(&meta, &Thresholds::default());
        assert_eq!(f.severity, Severity::Pass);
    }

    #[test]
    fn fresh_branch_passes() {
        let mut meta = make_test_metadata();
        let now = Utc::now();
        meta.snapshots = vec![Snapshot {
            snapshot_id: 99,
            timestamp_ms: now.timestamp_millis(),
            ..Default::default()
        }];
        meta.refs.insert(
            "audit".to_string(),
            SnapshotRef {
                snapshot_id: 99,
                ref_type: "branch".into(),
                max_ref_age_ms: None,
                max_snapshot_age_ms: None,
                min_snapshots_to_keep: None,
            },
        );
        let f = BranchHealthCheck.check(&meta, &Thresholds::default());
        assert_eq!(f.severity, Severity::Pass);
    }

    #[test]
    fn stale_branch_warns() {
        let mut meta = make_test_metadata();
        let old = Utc::now() - Duration::days(120);
        meta.snapshots = vec![Snapshot {
            snapshot_id: 50,
            timestamp_ms: old.timestamp_millis(),
            ..Default::default()
        }];
        meta.refs.insert(
            "old-audit".to_string(),
            SnapshotRef {
                snapshot_id: 50,
                ref_type: "branch".into(),
                max_ref_age_ms: None,
                max_snapshot_age_ms: None,
                min_snapshots_to_keep: None,
            },
        );
        let f = BranchHealthCheck.check(&meta, &Thresholds::default());
        assert!(f.severity >= Severity::Warning);
    }

    #[test]
    fn dangling_ref_is_critical() {
        let mut meta = make_test_metadata();
        meta.snapshots = vec![]; // no snapshots
        meta.refs.insert(
            "ghost".to_string(),
            SnapshotRef {
                snapshot_id: 999,
                ref_type: "tag".into(),
                max_ref_age_ms: None,
                max_snapshot_age_ms: None,
                min_snapshots_to_keep: None,
            },
        );
        let f = BranchHealthCheck.check(&meta, &Thresholds::default());
        assert_eq!(f.severity, Severity::Critical);
    }
}
