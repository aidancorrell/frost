//! Frost MCP server — tool implementation.

use crate::tools::*;
use frost_core::catalog;
use frost_core::config::FrostConfig;
use frost_core::fleet::{FleetInput, compute_fleet_report};
use frost_core::report::Severity;
use frost_core::{cost, engine, fix};
use rmcp::handler::server::wrapper::Parameters;
use rmcp::{tool, tool_router};

/// The frost MCP server. Exposes Iceberg table health tools to AI agents.
#[derive(Clone)]
pub struct FrostServer {
    config: FrostConfig,
}

impl FrostServer {
    pub fn new(config: FrostConfig) -> Self {
        Self { config }
    }

    /// Load table metadata from the configured catalog.
    async fn load_table(
        &self,
        table_identifier: &str,
    ) -> Result<frost_core::metadata::TableMetadata, String> {
        let provider = catalog::from_config(&self.config.catalog)
            .map_err(|e| format!("Catalog error: {}", e))?;
        provider
            .load_table(table_identifier)
            .await
            .map_err(|e| format!("Failed to load table '{}': {}", table_identifier, e))
    }

    /// Run health checks on an Iceberg table (public for testing).
    pub async fn run_check_table(&self, params: CheckTableParams) -> String {
        let metadata = match self.load_table(&params.table).await {
            Ok(m) => m,
            Err(e) => {
                return serde_json::to_string_pretty(&serde_json::json!({"error": e})).unwrap();
            }
        };

        let report = match params.checks {
            Some(ref ids) => {
                let id_refs: Vec<&str> = ids.iter().map(|s| s.as_str()).collect();
                engine::check_table_filtered(&metadata, &self.config.thresholds, &id_refs)
            }
            None => engine::check_table(&metadata, &self.config),
        };

        serde_json::to_string_pretty(&report).unwrap()
    }

    /// Summarize health across all tables (public for testing).
    pub async fn run_check_catalog(&self, params: CheckCatalogParams) -> String {
        let provider = match catalog::from_config(&self.config.catalog) {
            Ok(p) => p,
            Err(e) => {
                return serde_json::to_string_pretty(
                    &serde_json::json!({"error": format!("Catalog error: {}", e)}),
                )
                .unwrap();
            }
        };

        let tables = match provider.list_tables(params.namespace.as_deref()).await {
            Ok(t) => t,
            Err(e) => {
                return serde_json::to_string_pretty(
                    &serde_json::json!({"error": format!("Failed to list tables: {}", e)}),
                )
                .unwrap();
            }
        };

        let mut table_reports = Vec::new();
        let mut healthy = 0;
        let mut warning = 0;
        let mut critical = 0;

        for table_id in &tables {
            match provider.load_table(table_id).await {
                Ok(metadata) => {
                    let report = engine::check_table(&metadata, &self.config);
                    match report.overall.severity {
                        Severity::Pass => healthy += 1,
                        Severity::Warning => warning += 1,
                        Severity::Critical => critical += 1,
                    }
                    table_reports.push(TableBrief {
                        table_name: table_id.clone(),
                        severity: report.overall.severity.to_string(),
                        finding_count: report.findings.len(),
                        critical_count: report.overall.critical_count,
                        warning_count: report.overall.warning_count,
                    });
                }
                Err(e) => {
                    tracing::warn!("Failed to load table '{}': {}", table_id, e);
                    table_reports.push(TableBrief {
                        table_name: table_id.clone(),
                        severity: "ERROR".to_string(),
                        finding_count: 0,
                        critical_count: 0,
                        warning_count: 0,
                    });
                }
            }
        }

        // Sort: critical first, then warning, then pass.
        table_reports.sort_by(|a, b| {
            let order = |s: &str| match s {
                "CRITICAL" => 0,
                "WARNING" => 1,
                "ERROR" => 2,
                _ => 3,
            };
            order(&a.severity).cmp(&order(&b.severity))
        });

        let summary = CatalogSummary {
            tables_checked: tables.len(),
            tables_healthy: healthy,
            tables_warning: warning,
            tables_critical: critical,
            table_reports,
        };

        serde_json::to_string_pretty(&summary).unwrap()
    }

    /// Compute fleet-level signals across all tables in a catalog.
    pub async fn run_check_fleet(&self, params: CheckFleetParams) -> String {
        let provider = match catalog::from_config(&self.config.catalog) {
            Ok(p) => p,
            Err(e) => {
                return serde_json::to_string_pretty(
                    &serde_json::json!({"error": format!("Catalog error: {}", e)}),
                )
                .unwrap();
            }
        };

        let tables = match provider.list_tables(params.namespace.as_deref()).await {
            Ok(t) => t,
            Err(e) => {
                return serde_json::to_string_pretty(
                    &serde_json::json!({"error": format!("Failed to list tables: {}", e)}),
                )
                .unwrap();
            }
        };

        let mut inputs: Vec<FleetInput> = Vec::with_capacity(tables.len());
        let mut unreadable = 0usize;
        for table_id in &tables {
            match provider.load_table(table_id).await {
                Ok(metadata) => {
                    inputs.push(FleetInput::from_metadata(
                        table_id.clone(),
                        metadata,
                        &self.config,
                    ));
                }
                Err(e) => {
                    tracing::warn!("check_fleet: failed to load '{}': {}", table_id, e);
                    unreadable += 1;
                }
            }
        }

        let report = compute_fleet_report(inputs, unreadable, params.dormant_days);
        serde_json::to_string_pretty(&report).unwrap()
    }

    /// Generate a fix command (public for testing).
    pub async fn run_get_fix(&self, params: GetFixParams) -> String {
        let metadata = match self.load_table(&params.table).await {
            Ok(m) => m,
            Err(e) => {
                return serde_json::to_string_pretty(&serde_json::json!({"error": e})).unwrap();
            }
        };

        match fix::generate_fix(&metadata, &params.finding_id) {
            Some(cmd) => serde_json::to_string_pretty(&cmd).unwrap(),
            None => serde_json::to_string_pretty(&serde_json::json!({
                "error": format!("No fix available for finding '{}'", params.finding_id),
                "available_findings": [
                    "small_files", "snapshot_bloat", "orphan_files",
                    "delete_pressure", "metadata_size", "partition_skew",
                    "format_v1", "properties_drift", "partition_spec_evolution",
                    "sort_compliance", "stats_coverage"
                ]
            }))
            .unwrap(),
        }
    }

    /// Dry-run a fix: return the scope (what will change) without exposing
    /// the executable command. Lets agents reason about cost before
    /// committing.
    pub async fn run_dry_run_fix(&self, params: DryRunFixParams) -> String {
        let metadata = match self.load_table(&params.table).await {
            Ok(m) => m,
            Err(e) => {
                return serde_json::to_string_pretty(&serde_json::json!({"error": e})).unwrap();
            }
        };

        match fix::generate_fix(&metadata, &params.finding_id) {
            Some(cmd) => serde_json::to_string_pretty(&serde_json::json!({
                "finding_id": cmd.finding_id,
                "table_name": cmd.table_name,
                "description": cmd.description,
                "warnings": cmd.warnings,
                "scope": cmd.scope,
                "note": "This is a dry run — no command was returned. Call get_fix to get the executable command."
            }))
            .unwrap(),
            None => serde_json::to_string_pretty(&serde_json::json!({
                "error": format!("No fix available for finding '{}'", params.finding_id),
            }))
            .unwrap(),
        }
    }

    /// Estimate cost waste (public for testing).
    pub async fn run_get_cost_report(&self, params: GetCostReportParams) -> String {
        let metadata = match self.load_table(&params.table).await {
            Ok(m) => m,
            Err(e) => {
                return serde_json::to_string_pretty(&serde_json::json!({"error": e})).unwrap();
            }
        };

        let report = cost::estimate_cost(&metadata, &self.config.cost);
        serde_json::to_string_pretty(&report).unwrap()
    }

    /// Query watch mode state (public for testing).
    pub async fn run_watch_status(&self, params: WatchStatusParams) -> String {
        use frost_core::watch::WatchDb;

        // Try to open the watch database.
        let db = match WatchDb::open(&self.config.watch.sqlite_path) {
            Ok(db) => db,
            Err(_) => {
                // No database — watch mode hasn't been run.
                return serde_json::to_string_pretty(&WatchStatusResponse {
                    status: "not_configured".to_string(),
                    message: format!(
                        "Watch mode database not found at '{}'. Start it with: frost watch",
                        self.config.watch.sqlite_path
                    ),
                    tables_watched: None,
                    table_health: None,
                    recent_alerts: None,
                })
                .unwrap();
            }
        };

        // Get latest health state.
        let latest = if let Some(ref table) = params.table {
            db.get_latest_report(table)
                .unwrap_or(None)
                .into_iter()
                .collect::<Vec<_>>()
        } else {
            db.get_all_latest().unwrap_or_default()
        };

        if latest.is_empty() {
            return serde_json::to_string_pretty(&WatchStatusResponse {
                status: "no_data".to_string(),
                message: "Watch database exists but contains no check results. Run 'frost watch' to start monitoring.".to_string(),
                tables_watched: Some(0),
                table_health: None,
                recent_alerts: None,
            })
            .unwrap();
        }

        let trend_days = params.trend_days.unwrap_or(7);
        // Default: compute trend if a single table was requested, skip if
        // the caller asked for the whole catalog (trend computation runs
        // per table).
        let want_trend = params
            .include_trend
            .unwrap_or_else(|| params.table.is_some());

        let table_health: Vec<WatchTableHealth> = latest
            .iter()
            .map(|r| {
                let trend = if want_trend {
                    db.compute_trend(&r.table_name, trend_days).ok()
                } else {
                    None
                };
                WatchTableHealth {
                    table_name: r.table_name.clone(),
                    severity: r.severity.clone(),
                    finding_count: r.finding_count,
                    last_checked: r.checked_at.to_rfc3339(),
                    trend,
                }
            })
            .collect();

        let alerts = db
            .get_alerts(params.table.as_deref(), 10)
            .unwrap_or_default();
        let alert_briefs: Vec<WatchAlertBrief> = alerts
            .iter()
            .map(|a| WatchAlertBrief {
                table_name: a.table_name.clone(),
                message: a.message.clone(),
                alerted_at: a.alerted_at.to_rfc3339(),
            })
            .collect();

        let response = WatchStatusResponse {
            status: "has_data".to_string(),
            message: format!(
                "{} table(s) monitored, {} recent alert(s)",
                table_health.len(),
                alert_briefs.len()
            ),
            tables_watched: Some(table_health.len()),
            table_health: Some(table_health),
            recent_alerts: if alert_briefs.is_empty() {
                None
            } else {
                Some(alert_briefs)
            },
        };

        serde_json::to_string_pretty(&response).unwrap()
    }
}

#[tool_router(server_handler)]
impl FrostServer {
    #[tool(
        description = "Run health checks on an Iceberg table. Returns findings with severity, impact, and fix commands."
    )]
    async fn check_table(&self, Parameters(params): Parameters<CheckTableParams>) -> String {
        self.run_check_table(params).await
    }

    #[tool(description = "Summarize health across all tables in a catalog, sorted by severity.")]
    async fn check_catalog(&self, Parameters(params): Parameters<CheckCatalogParams>) -> String {
        self.run_check_catalog(params).await
    }

    #[tool(
        description = "Fleet-level signals across all tables: per-namespace rollup, dormant tables (no recent commits), unpartitioned tables, format-v1 tables, and the top-N offenders. Use this when you own a fleet rather than a single table."
    )]
    async fn check_fleet(&self, Parameters(params): Parameters<CheckFleetParams>) -> String {
        self.run_check_fleet(params).await
    }

    #[tool(
        description = "Generate a Spark SQL fix command for a specific health finding on a table. Includes scope (estimated_files, estimated_bytes, estimated_partitions, estimated_snapshots_expired)."
    )]
    async fn get_fix(&self, Parameters(params): Parameters<GetFixParams>) -> String {
        self.run_get_fix(params).await
    }

    #[tool(
        description = "Dry-run a fix: returns the scope (estimated files/bytes/partitions affected) without an executable command. Use to reason about fix cost before committing."
    )]
    async fn dry_run_fix(&self, Parameters(params): Parameters<DryRunFixParams>) -> String {
        self.run_dry_run_fix(params).await
    }

    #[tool(description = "Estimate monthly cost waste from health issues on an Iceberg table.")]
    async fn get_cost_report(&self, Parameters(params): Parameters<GetCostReportParams>) -> String {
        self.run_get_cost_report(params).await
    }

    #[tool(description = "Query watch mode state for recent alerts and health trends.")]
    async fn watch_status(&self, Parameters(params): Parameters<WatchStatusParams>) -> String {
        self.run_watch_status(params).await
    }
}
