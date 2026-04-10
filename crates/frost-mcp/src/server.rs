//! Frost MCP server — tool implementation.

use crate::tools::*;
use frost_core::catalog;
use frost_core::config::FrostConfig;
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
                return serde_json::to_string_pretty(&serde_json::json!({"error": e})).unwrap()
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

    /// Generate a fix command (public for testing).
    pub async fn run_get_fix(&self, params: GetFixParams) -> String {
        let metadata = match self.load_table(&params.table).await {
            Ok(m) => m,
            Err(e) => {
                return serde_json::to_string_pretty(&serde_json::json!({"error": e})).unwrap()
            }
        };

        match fix::generate_fix(&metadata, &params.finding_id) {
            Some(cmd) => serde_json::to_string_pretty(&cmd).unwrap(),
            None => serde_json::to_string_pretty(&serde_json::json!({
                "error": format!("No fix available for finding '{}'", params.finding_id),
                "available_findings": [
                    "small_files", "snapshot_bloat", "orphan_files",
                    "delete_pressure", "metadata_size", "partition_skew"
                ]
            }))
            .unwrap(),
        }
    }

    /// Estimate cost waste (public for testing).
    pub async fn run_get_cost_report(&self, params: GetCostReportParams) -> String {
        let metadata = match self.load_table(&params.table).await {
            Ok(m) => m,
            Err(e) => {
                return serde_json::to_string_pretty(&serde_json::json!({"error": e})).unwrap()
            }
        };

        let report = cost::estimate_cost(&metadata, &self.config.cost);
        serde_json::to_string_pretty(&report).unwrap()
    }

    /// Query watch mode state (public for testing).
    pub async fn run_watch_status(&self, params: WatchStatusParams) -> String {
        let response = WatchStatusResponse {
            status: "not_running".to_string(),
            message: format!(
                "Watch mode is not yet active. Start it with: frost watch{}",
                params
                    .table
                    .as_ref()
                    .map(|t| format!(" --table {}", t))
                    .unwrap_or_default()
            ),
        };
        serde_json::to_string_pretty(&response).unwrap()
    }
}

#[tool_router(server_handler)]
impl FrostServer {
    #[tool(description = "Run health checks on an Iceberg table. Returns findings with severity, impact, and fix commands.")]
    async fn check_table(
        &self,
        Parameters(params): Parameters<CheckTableParams>,
    ) -> String {
        self.run_check_table(params).await
    }

    #[tool(description = "Summarize health across all tables in a catalog, sorted by severity.")]
    async fn check_catalog(
        &self,
        Parameters(params): Parameters<CheckCatalogParams>,
    ) -> String {
        self.run_check_catalog(params).await
    }

    #[tool(description = "Generate a Spark SQL fix command for a specific health finding on a table.")]
    async fn get_fix(
        &self,
        Parameters(params): Parameters<GetFixParams>,
    ) -> String {
        self.run_get_fix(params).await
    }

    #[tool(description = "Estimate monthly cost waste from health issues on an Iceberg table.")]
    async fn get_cost_report(
        &self,
        Parameters(params): Parameters<GetCostReportParams>,
    ) -> String {
        self.run_get_cost_report(params).await
    }

    #[tool(description = "Query watch mode state for recent alerts and health trends.")]
    async fn watch_status(
        &self,
        Parameters(params): Parameters<WatchStatusParams>,
    ) -> String {
        self.run_watch_status(params).await
    }
}
