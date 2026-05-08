//! MCP tool parameter and response types.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

// --- Tool Parameters ---

#[derive(Debug, Deserialize, JsonSchema)]
pub struct CheckTableParams {
    /// Table identifier (e.g., "db.events").
    pub table: String,
    /// Optional list of check IDs to run (default: all).
    /// Valid IDs: small_files, snapshot_bloat, orphan_files, partition_skew,
    /// delete_pressure, schema_history, metadata_size, sort_order, freshness.
    #[serde(default)]
    pub checks: Option<Vec<String>>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct CheckCatalogParams {
    /// Optional namespace filter (e.g., "production").
    #[serde(default)]
    pub namespace: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct CheckFleetParams {
    /// Optional namespace filter.
    #[serde(default)]
    pub namespace: Option<String>,
    /// Days since last commit before a table is "dormant". Default: 90.
    #[serde(default)]
    pub dormant_days: Option<i64>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetFixParams {
    /// Table identifier (e.g., "db.events").
    pub table: String,
    /// Finding ID to generate a fix for (e.g., "small_files", "snapshot_bloat").
    pub finding_id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct DryRunFixParams {
    /// Table identifier (e.g., "db.events").
    pub table: String,
    /// Finding ID to dry-run (e.g., "small_files").
    pub finding_id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetCostReportParams {
    /// Table identifier (e.g., "db.events").
    pub table: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct WatchStatusParams {
    /// Optional table identifier (default: all watched tables).
    #[serde(default)]
    pub table: Option<String>,
    /// If true, include rolling trend signals (improving/degrading/flapping)
    /// for the table(s) over the past `trend_days` window. Default: true
    /// when `table` is set, false otherwise (computing trends across all
    /// tables can be expensive).
    #[serde(default)]
    pub include_trend: Option<bool>,
    /// Lookback window for trend computation. Default: 7.
    #[serde(default)]
    pub trend_days: Option<i64>,
}

// --- Tool Responses ---

#[derive(Debug, Serialize)]
pub struct CatalogSummary {
    pub tables_checked: usize,
    pub tables_healthy: usize,
    pub tables_warning: usize,
    pub tables_critical: usize,
    pub table_reports: Vec<TableBrief>,
}

#[derive(Debug, Serialize)]
pub struct TableBrief {
    pub table_name: String,
    pub severity: String,
    pub finding_count: usize,
    pub critical_count: usize,
    pub warning_count: usize,
}

#[derive(Debug, Serialize)]
pub struct WatchStatusResponse {
    pub status: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tables_watched: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub table_health: Option<Vec<WatchTableHealth>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recent_alerts: Option<Vec<WatchAlertBrief>>,
}

#[derive(Debug, Serialize)]
pub struct WatchTableHealth {
    pub table_name: String,
    pub severity: String,
    pub finding_count: usize,
    pub last_checked: String,
    /// Rolling trend classification — populated only when the caller asks
    /// for it (or the request was for a single table).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trend: Option<frost_core::watch::TableTrend>,
}

#[derive(Debug, Serialize)]
pub struct WatchAlertBrief {
    pub table_name: String,
    pub message: String,
    pub alerted_at: String,
}
