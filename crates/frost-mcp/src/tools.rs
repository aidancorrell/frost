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
pub struct GetFixParams {
    /// Table identifier (e.g., "db.events").
    pub table: String,
    /// Finding ID to generate a fix for (e.g., "small_files", "snapshot_bloat").
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
}
