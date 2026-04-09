//! The frost health check engine.
//!
//! Runs all enabled checks against table metadata and produces a report.

use crate::checks::{self, HealthCheck};
use crate::config::{FrostConfig, Thresholds};
use crate::metadata::TableMetadata;
use crate::report::{Finding, HealthReport, TableSummary};
use chrono::Utc;

/// Run all health checks against table metadata and produce a report.
pub fn check_table(metadata: &TableMetadata, config: &FrostConfig) -> HealthReport {
    check_table_with_checks(metadata, &config.thresholds, &checks::all_checks())
}

/// Run a specific set of checks (useful for filtering or testing).
pub fn check_table_with_checks(
    metadata: &TableMetadata,
    thresholds: &Thresholds,
    checks: &[Box<dyn HealthCheck>],
) -> HealthReport {
    let findings: Vec<Finding> = checks
        .iter()
        .map(|check| check.check(metadata, thresholds))
        .collect();

    let overall = HealthReport::compute_overall(&findings);

    let total_size_bytes: u64 = metadata.data_files.iter().map(|f| f.file_size_bytes).sum();
    let total_record_count: u64 = metadata.data_files.iter().map(|f| f.record_count).sum();

    HealthReport {
        table_name: metadata.table_name.clone(),
        location: metadata.location.clone(),
        summary: TableSummary {
            snapshot_count: metadata.snapshots.len() as u64,
            data_file_count: metadata.data_files.len() as u64,
            total_size_bytes,
            total_record_count,
        },
        findings,
        overall,
        generated_at: Utc::now(),
    }
}

/// Run checks against a specific list of check IDs (for filtering).
pub fn check_table_filtered(
    metadata: &TableMetadata,
    thresholds: &Thresholds,
    check_ids: &[&str],
) -> HealthReport {
    let all = checks::all_checks();
    let filtered: Vec<_> = all
        .into_iter()
        .filter(|c| check_ids.contains(&c.id()))
        .collect();
    check_table_with_checks(metadata, thresholds, &filtered)
}
