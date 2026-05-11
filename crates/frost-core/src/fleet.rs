//! Fleet-level signals — health computed across many tables.
//!
//! Per-table checks miss problems that only show up when you zoom out:
//! tables with no commits in 90 days, namespaces with too many tables,
//! and tables that are silently abandoned (no recent snapshots, but no
//! one's noticed). A Staff DE running a fleet needs these signals as
//! much as the per-table ones.

use crate::engine;
use crate::metadata::TableMetadata;
use crate::report::{HealthReport, Severity};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Aggregate fleet-level findings across all checked tables.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FleetReport {
    /// Total tables scanned.
    pub tables_scanned: usize,
    /// Tables that failed to load (catalog/parse errors).
    pub tables_unreadable: usize,
    /// Per-namespace counts.
    pub namespaces: Vec<NamespaceSummary>,
    /// Tables that haven't had a commit in `dormant_days` or longer.
    pub dormant_tables: Vec<DormantTable>,
    /// Tables with NO partition spec declared (whole-table scans).
    pub unpartitioned_tables: Vec<String>,
    /// Tables on Iceberg format-version 1.
    pub format_v1_tables: Vec<String>,
    /// Per-severity rollup.
    pub by_severity: HashMap<String, usize>,
    /// Top-N tables sorted by criticality.
    pub top_offenders: Vec<TableBrief>,
    /// Generated-at timestamp.
    pub generated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NamespaceSummary {
    pub namespace: String,
    pub table_count: usize,
    pub critical: usize,
    pub warning: usize,
    pub healthy: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DormantTable {
    pub table_name: String,
    pub days_since_last_commit: i64,
    pub snapshot_count: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TableBrief {
    pub table_name: String,
    pub severity: String,
    pub critical_count: usize,
    pub warning_count: usize,
}

/// Threshold for "dormant" — default 90 days since last commit.
const DEFAULT_DORMANT_DAYS: i64 = 90;

/// Compute fleet signals from per-table reports + their metadata.
///
/// Inputs are keyed by table identifier so we can correlate metadata
/// (for namespace, partition spec, format version, freshness) with the
/// computed report.
pub fn compute_fleet_report(
    inputs: Vec<FleetInput>,
    unreadable_count: usize,
    dormant_days: Option<i64>,
) -> FleetReport {
    let dormant_threshold = dormant_days.unwrap_or(DEFAULT_DORMANT_DAYS);
    let now = Utc::now();

    let mut by_namespace: HashMap<String, NamespaceSummary> = HashMap::new();
    let mut by_severity: HashMap<String, usize> = HashMap::new();
    let mut dormant_tables = Vec::new();
    let mut unpartitioned_tables = Vec::new();
    let mut format_v1_tables = Vec::new();
    let mut briefs: Vec<TableBrief> = Vec::new();

    for input in &inputs {
        // Namespace = everything before the last dot.
        let namespace = input
            .table_name
            .rsplit_once('.')
            .map(|(ns, _)| ns.to_string())
            .unwrap_or_else(|| "<root>".to_string());

        let entry = by_namespace
            .entry(namespace.clone())
            .or_insert(NamespaceSummary {
                namespace,
                table_count: 0,
                critical: 0,
                warning: 0,
                healthy: 0,
            });
        entry.table_count += 1;
        match input.report.overall.severity {
            Severity::Critical => entry.critical += 1,
            Severity::Warning => entry.warning += 1,
            Severity::Pass => entry.healthy += 1,
        }

        let sev_key = input.report.overall.severity.to_string();
        *by_severity.entry(sev_key).or_default() += 1;

        // Dormancy.
        if let Some(latest) = input.metadata.snapshots.last() {
            let age_days = (now - latest.timestamp()).num_days();
            if age_days >= dormant_threshold {
                dormant_tables.push(DormantTable {
                    table_name: input.table_name.clone(),
                    days_since_last_commit: age_days,
                    snapshot_count: input.metadata.snapshots.len() as u64,
                });
            }
        }

        // Unpartitioned.
        if input.metadata.partition_spec.fields.is_empty() {
            unpartitioned_tables.push(input.table_name.clone());
        }

        // v1 tables.
        if input.metadata.format_version == 1 {
            format_v1_tables.push(input.table_name.clone());
        }

        briefs.push(TableBrief {
            table_name: input.table_name.clone(),
            severity: input.report.overall.severity.to_string(),
            critical_count: input.report.overall.critical_count,
            warning_count: input.report.overall.warning_count,
        });
    }

    // Top offenders: sort by (critical desc, warning desc).
    briefs.sort_by(|a, b| {
        b.critical_count
            .cmp(&a.critical_count)
            .then_with(|| b.warning_count.cmp(&a.warning_count))
    });
    let top_offenders: Vec<TableBrief> = briefs.into_iter().take(10).collect();

    let mut namespaces: Vec<NamespaceSummary> = by_namespace.into_values().collect();
    namespaces.sort_by_key(|n| std::cmp::Reverse(n.critical));

    dormant_tables.sort_by_key(|d| std::cmp::Reverse(d.days_since_last_commit));

    FleetReport {
        tables_scanned: inputs.len(),
        tables_unreadable: unreadable_count,
        namespaces,
        dormant_tables,
        unpartitioned_tables,
        format_v1_tables,
        by_severity,
        top_offenders,
        generated_at: now,
    }
}

/// Per-table input to the fleet aggregator.
#[derive(Debug, Clone)]
pub struct FleetInput {
    pub table_name: String,
    pub metadata: TableMetadata,
    pub report: HealthReport,
}

impl FleetInput {
    pub fn from_metadata(
        table_name: String,
        metadata: TableMetadata,
        config: &crate::config::FrostConfig,
    ) -> Self {
        let report = engine::check_table(&metadata, config);
        Self {
            table_name,
            metadata,
            report,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::report::OverallStatus;
    use crate::test_helpers::make_test_metadata;
    use chrono::Duration;

    fn report_with(severity: Severity, crit: usize, warn: usize) -> HealthReport {
        HealthReport {
            table_name: "t".to_string(),
            location: "s3://x".to_string(),
            summary: crate::report::TableSummary {
                snapshot_count: 1,
                data_file_count: 1,
                total_size_bytes: 0,
                total_record_count: 0,
            },
            findings: vec![],
            overall: OverallStatus {
                severity,
                pass_count: 0,
                warning_count: warn,
                critical_count: crit,
            },
            generated_at: Utc::now(),
        }
    }

    #[test]
    fn aggregates_by_namespace() {
        let mut t1 = make_test_metadata();
        t1.table_name = "ns1.events".into();
        let mut t2 = make_test_metadata();
        t2.table_name = "ns1.users".into();
        let mut t3 = make_test_metadata();
        t3.table_name = "ns2.logs".into();

        let inputs = vec![
            FleetInput {
                table_name: t1.table_name.clone(),
                metadata: t1,
                report: report_with(Severity::Critical, 1, 0),
            },
            FleetInput {
                table_name: t2.table_name.clone(),
                metadata: t2,
                report: report_with(Severity::Warning, 0, 2),
            },
            FleetInput {
                table_name: t3.table_name.clone(),
                metadata: t3,
                report: report_with(Severity::Pass, 0, 0),
            },
        ];

        let report = compute_fleet_report(inputs, 0, None);
        assert_eq!(report.tables_scanned, 3);
        assert_eq!(report.namespaces.len(), 2);
        // ns1 has 1 critical, 1 warning
        let ns1 = report
            .namespaces
            .iter()
            .find(|n| n.namespace == "ns1")
            .unwrap();
        assert_eq!(ns1.critical, 1);
        assert_eq!(ns1.warning, 1);
    }

    #[test]
    fn flags_dormant_tables() {
        let mut t = make_test_metadata();
        t.table_name = "ns.old".into();
        t.snapshots[0].timestamp_ms = (Utc::now() - Duration::days(120)).timestamp_millis();

        let inputs = vec![FleetInput {
            table_name: t.table_name.clone(),
            metadata: t,
            report: report_with(Severity::Pass, 0, 0),
        }];

        let report = compute_fleet_report(inputs, 0, None);
        assert_eq!(report.dormant_tables.len(), 1);
        assert!(report.dormant_tables[0].days_since_last_commit >= 90);
    }

    #[test]
    fn flags_v1_tables() {
        let mut t = make_test_metadata();
        t.table_name = "ns.v1".into();
        t.format_version = 1;
        let inputs = vec![FleetInput {
            table_name: t.table_name.clone(),
            metadata: t,
            report: report_with(Severity::Warning, 0, 1),
        }];
        let report = compute_fleet_report(inputs, 0, None);
        assert_eq!(report.format_v1_tables, vec!["ns.v1"]);
    }
}
