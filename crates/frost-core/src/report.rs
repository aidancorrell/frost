//! Structured health report types.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// A complete health report for one table.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthReport {
    pub table_name: String,
    pub location: String,
    pub summary: TableSummary,
    pub findings: Vec<Finding>,
    pub overall: OverallStatus,
    pub generated_at: DateTime<Utc>,
}

/// High-level table stats shown at the top of the report.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TableSummary {
    pub snapshot_count: u64,
    pub data_file_count: u64,
    pub total_size_bytes: u64,
    pub total_record_count: u64,
}

/// A single health finding (one check's output).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Finding {
    /// Machine-readable check identifier (e.g., "small_files", "snapshot_bloat").
    pub check_id: String,
    /// Human-readable check name.
    pub check_name: String,
    pub severity: Severity,
    /// One-line description of what was found.
    pub message: String,
    /// Why this matters.
    pub impact: String,
    /// Suggested fix (human-readable).
    pub fix_suggestion: Option<String>,
    /// Machine-executable fix command (Spark SQL CALL statement or similar).
    pub fix_command: Option<String>,
    /// Estimated cost savings if the issue is fixed.
    pub estimated_savings: Option<String>,
    /// Additional structured details (check-specific).
    pub details: serde_json::Value,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    /// Everything looks good.
    Pass,
    /// Something to keep an eye on.
    Warning,
    /// Needs attention.
    Critical,
}

impl std::fmt::Display for Severity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Pass => write!(f, "PASS"),
            Self::Warning => write!(f, "WARNING"),
            Self::Critical => write!(f, "CRITICAL"),
        }
    }
}

/// Overall status derived from the worst finding.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OverallStatus {
    pub severity: Severity,
    pub pass_count: usize,
    pub warning_count: usize,
    pub critical_count: usize,
}

impl HealthReport {
    /// Build overall status from findings.
    pub fn compute_overall(findings: &[Finding]) -> OverallStatus {
        let pass_count = findings
            .iter()
            .filter(|f| f.severity == Severity::Pass)
            .count();
        let warning_count = findings
            .iter()
            .filter(|f| f.severity == Severity::Warning)
            .count();
        let critical_count = findings
            .iter()
            .filter(|f| f.severity == Severity::Critical)
            .count();

        let severity = if critical_count > 0 {
            Severity::Critical
        } else if warning_count > 0 {
            Severity::Warning
        } else {
            Severity::Pass
        };

        OverallStatus {
            severity,
            pass_count,
            warning_count,
            critical_count,
        }
    }
}
