//! Watch mode — periodic table health monitoring with persistent state.
//!
//! Runs health checks on a schedule, stores results in SQLite, detects
//! health regressions, and fires webhook alerts when things change.

use crate::catalog;
use crate::config::FrostConfig;
use crate::engine;
use crate::report::{HealthReport, Severity};
use chrono::{DateTime, Utc};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use std::path::Path;

// ---------------------------------------------------------------------------
// Config
// ---------------------------------------------------------------------------

/// Configuration for watch mode.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct WatchConfig {
    /// Check interval (e.g., "6h", "30m", "1h"). Default: "6h".
    pub interval: String,
    /// Webhook URL for alerts (Slack, generic HTTP POST). Optional.
    pub webhook_url: Option<String>,
    /// Path to the SQLite database for state tracking.
    pub sqlite_path: String,
    /// Optional namespace filter — only watch tables in this namespace.
    pub namespace: Option<String>,
}

impl Default for WatchConfig {
    fn default() -> Self {
        Self {
            interval: "6h".to_string(),
            webhook_url: None,
            sqlite_path: "./frost-watch.db".to_string(),
            namespace: None,
        }
    }
}

/// Parse an interval string like "6h", "30m", "1d" into seconds.
pub fn parse_interval(s: &str) -> Result<u64, WatchError> {
    let s = s.trim();
    if s.is_empty() {
        return Err(WatchError::Config("empty interval".to_string()));
    }

    let (num_str, unit) = s.split_at(s.len() - 1);
    let num: u64 = num_str
        .parse()
        .map_err(|_| WatchError::Config(format!("invalid interval: {s}")))?;

    match unit {
        "s" => Ok(num),
        "m" => Ok(num * 60),
        "h" => Ok(num * 3600),
        "d" => Ok(num * 86400),
        _ => Err(WatchError::Config(format!(
            "unknown interval unit '{unit}', expected s/m/h/d"
        ))),
    }
}

// ---------------------------------------------------------------------------
// SQLite state
// ---------------------------------------------------------------------------

/// Persistent watch state backed by SQLite.
pub struct WatchDb {
    conn: Connection,
}

/// A stored health report summary.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredReport {
    pub table_name: String,
    pub severity: String,
    pub finding_count: usize,
    pub critical_count: usize,
    pub warning_count: usize,
    pub checked_at: DateTime<Utc>,
    pub report_json: String,
}

/// A health change that triggered an alert.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WatchAlert {
    pub table_name: String,
    pub previous_severity: String,
    pub current_severity: String,
    pub message: String,
    pub new_findings: Vec<String>,
    pub resolved_findings: Vec<String>,
    pub alerted_at: DateTime<Utc>,
}

impl WatchDb {
    /// Open (or create) the SQLite database at the given path.
    pub fn open(path: &str) -> Result<Self, WatchError> {
        // Create parent directories if needed.
        if let Some(parent) = Path::new(path).parent()
            && !parent.as_os_str().is_empty()
        {
            std::fs::create_dir_all(parent).map_err(WatchError::Io)?;
        }

        let conn = Connection::open(path).map_err(WatchError::Sqlite)?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS check_history (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                table_name TEXT NOT NULL,
                severity TEXT NOT NULL,
                finding_count INTEGER NOT NULL,
                critical_count INTEGER NOT NULL,
                warning_count INTEGER NOT NULL,
                report_json TEXT NOT NULL,
                checked_at TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS alerts (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                table_name TEXT NOT NULL,
                previous_severity TEXT NOT NULL,
                current_severity TEXT NOT NULL,
                message TEXT NOT NULL,
                new_findings TEXT NOT NULL,
                resolved_findings TEXT NOT NULL,
                alerted_at TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_history_table ON check_history(table_name, checked_at);
            CREATE INDEX IF NOT EXISTS idx_alerts_table ON alerts(table_name, alerted_at);",
        )
        .map_err(WatchError::Sqlite)?;

        Ok(Self { conn })
    }

    /// Open an in-memory database (for testing).
    pub fn open_in_memory() -> Result<Self, WatchError> {
        let conn = Connection::open_in_memory().map_err(WatchError::Sqlite)?;
        let db = Self { conn };
        db.conn
            .execute_batch(
                "CREATE TABLE IF NOT EXISTS check_history (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                table_name TEXT NOT NULL,
                severity TEXT NOT NULL,
                finding_count INTEGER NOT NULL,
                critical_count INTEGER NOT NULL,
                warning_count INTEGER NOT NULL,
                report_json TEXT NOT NULL,
                checked_at TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS alerts (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                table_name TEXT NOT NULL,
                previous_severity TEXT NOT NULL,
                current_severity TEXT NOT NULL,
                message TEXT NOT NULL,
                new_findings TEXT NOT NULL,
                resolved_findings TEXT NOT NULL,
                alerted_at TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_history_table ON check_history(table_name, checked_at);
            CREATE INDEX IF NOT EXISTS idx_alerts_table ON alerts(table_name, alerted_at);",
            )
            .map_err(WatchError::Sqlite)?;
        Ok(db)
    }

    /// Store a health report.
    pub fn store_report(&self, report: &HealthReport) -> Result<(), WatchError> {
        let report_json =
            serde_json::to_string(report).map_err(|e| WatchError::Other(e.to_string()))?;

        self.conn
            .execute(
                "INSERT INTO check_history (table_name, severity, finding_count, critical_count, warning_count, report_json, checked_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    report.table_name,
                    report.overall.severity.to_string(),
                    report.findings.len(),
                    report.overall.critical_count,
                    report.overall.warning_count,
                    report_json,
                    report.generated_at.to_rfc3339(),
                ],
            )
            .map_err(WatchError::Sqlite)?;

        Ok(())
    }

    /// Store an alert.
    pub fn store_alert(&self, alert: &WatchAlert) -> Result<(), WatchError> {
        self.conn
            .execute(
                "INSERT INTO alerts (table_name, previous_severity, current_severity, message, new_findings, resolved_findings, alerted_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    alert.table_name,
                    alert.previous_severity,
                    alert.current_severity,
                    alert.message,
                    serde_json::to_string(&alert.new_findings).unwrap_or_default(),
                    serde_json::to_string(&alert.resolved_findings).unwrap_or_default(),
                    alert.alerted_at.to_rfc3339(),
                ],
            )
            .map_err(WatchError::Sqlite)?;

        Ok(())
    }

    /// Get the most recent report for a table.
    pub fn get_latest_report(&self, table_name: &str) -> Result<Option<StoredReport>, WatchError> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT table_name, severity, finding_count, critical_count, warning_count, report_json, checked_at
                 FROM check_history WHERE table_name = ?1
                 ORDER BY checked_at DESC LIMIT 1",
            )
            .map_err(WatchError::Sqlite)?;

        let result = stmt
            .query_row(params![table_name], |row| {
                Ok(StoredReport {
                    table_name: row.get(0)?,
                    severity: row.get(1)?,
                    finding_count: row.get(2)?,
                    critical_count: row.get(3)?,
                    warning_count: row.get(4)?,
                    report_json: row.get(5)?,
                    checked_at: row.get::<_, String>(6).map(|s| {
                        DateTime::parse_from_rfc3339(&s)
                            .unwrap()
                            .with_timezone(&Utc)
                    })?,
                })
            })
            .optional()
            .map_err(WatchError::Sqlite)?;

        Ok(result)
    }

    /// Get check history for a table, most recent first.
    pub fn get_history(
        &self,
        table_name: &str,
        limit: usize,
    ) -> Result<Vec<StoredReport>, WatchError> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT table_name, severity, finding_count, critical_count, warning_count, report_json, checked_at
                 FROM check_history WHERE table_name = ?1
                 ORDER BY checked_at DESC LIMIT ?2",
            )
            .map_err(WatchError::Sqlite)?;

        let rows = stmt
            .query_map(params![table_name, limit], |row| {
                Ok(StoredReport {
                    table_name: row.get(0)?,
                    severity: row.get(1)?,
                    finding_count: row.get(2)?,
                    critical_count: row.get(3)?,
                    warning_count: row.get(4)?,
                    report_json: row.get(5)?,
                    checked_at: row.get::<_, String>(6).map(|s| {
                        DateTime::parse_from_rfc3339(&s)
                            .unwrap()
                            .with_timezone(&Utc)
                    })?,
                })
            })
            .map_err(WatchError::Sqlite)?;

        let mut results = Vec::new();
        for row in rows {
            results.push(row.map_err(WatchError::Sqlite)?);
        }
        Ok(results)
    }

    /// Get recent alerts, optionally filtered by table.
    pub fn get_alerts(
        &self,
        table_name: Option<&str>,
        limit: usize,
    ) -> Result<Vec<WatchAlert>, WatchError> {
        let (sql, table_param) = match table_name {
            Some(t) => (
                "SELECT table_name, previous_severity, current_severity, message, new_findings, resolved_findings, alerted_at
                 FROM alerts WHERE table_name = ?1
                 ORDER BY alerted_at DESC LIMIT ?2",
                Some(t.to_string()),
            ),
            None => (
                "SELECT table_name, previous_severity, current_severity, message, new_findings, resolved_findings, alerted_at
                 FROM alerts
                 ORDER BY alerted_at DESC LIMIT ?1",
                None,
            ),
        };

        let mut stmt = self.conn.prepare(sql).map_err(WatchError::Sqlite)?;

        let rows = if let Some(ref t) = table_param {
            stmt.query_map(params![t, limit], map_alert_row)
                .map_err(WatchError::Sqlite)?
        } else {
            stmt.query_map(params![limit], map_alert_row)
                .map_err(WatchError::Sqlite)?
        };

        let mut results = Vec::new();
        for row in rows {
            results.push(row.map_err(WatchError::Sqlite)?);
        }
        Ok(results)
    }

    /// Get a summary of the latest health state for all watched tables.
    pub fn get_all_latest(&self) -> Result<Vec<StoredReport>, WatchError> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT h.table_name, h.severity, h.finding_count, h.critical_count, h.warning_count, h.report_json, h.checked_at
                 FROM check_history h
                 INNER JOIN (
                     SELECT table_name, MAX(checked_at) as max_checked
                     FROM check_history GROUP BY table_name
                 ) latest ON h.table_name = latest.table_name AND h.checked_at = latest.max_checked
                 ORDER BY h.table_name",
            )
            .map_err(WatchError::Sqlite)?;

        let rows = stmt
            .query_map([], |row| {
                Ok(StoredReport {
                    table_name: row.get(0)?,
                    severity: row.get(1)?,
                    finding_count: row.get(2)?,
                    critical_count: row.get(3)?,
                    warning_count: row.get(4)?,
                    report_json: row.get(5)?,
                    checked_at: row.get::<_, String>(6).map(|s| {
                        DateTime::parse_from_rfc3339(&s)
                            .unwrap()
                            .with_timezone(&Utc)
                    })?,
                })
            })
            .map_err(WatchError::Sqlite)?;

        let mut results = Vec::new();
        for row in rows {
            results.push(row.map_err(WatchError::Sqlite)?);
        }
        Ok(results)
    }
}

fn map_alert_row(row: &rusqlite::Row) -> rusqlite::Result<WatchAlert> {
    Ok(WatchAlert {
        table_name: row.get(0)?,
        previous_severity: row.get(1)?,
        current_severity: row.get(2)?,
        message: row.get(3)?,
        new_findings: row
            .get::<_, String>(4)
            .map(|s| serde_json::from_str(&s).unwrap_or_default())?,
        resolved_findings: row
            .get::<_, String>(5)
            .map(|s| serde_json::from_str(&s).unwrap_or_default())?,
        alerted_at: row.get::<_, String>(6).map(|s| {
            DateTime::parse_from_rfc3339(&s)
                .unwrap()
                .with_timezone(&Utc)
        })?,
    })
}

// ---------------------------------------------------------------------------
// Watch loop
// ---------------------------------------------------------------------------

/// Result of a single watch cycle.
#[derive(Debug, Clone, Serialize)]
pub struct WatchCycleResult {
    pub tables_checked: usize,
    pub alerts_fired: usize,
    pub alerts: Vec<WatchAlert>,
}

/// Run a single watch cycle: check all tables, compare to previous, fire alerts.
pub async fn run_watch_cycle(
    config: &FrostConfig,
    db: &WatchDb,
) -> Result<WatchCycleResult, WatchError> {
    let provider = catalog::from_config(&config.catalog)
        .map_err(|e| WatchError::Other(format!("Catalog error: {e}")))?;

    let tables = provider
        .list_tables(config.watch.namespace.as_deref())
        .await
        .map_err(|e| WatchError::Other(format!("Failed to list tables: {e}")))?;

    let mut alerts = Vec::new();

    for table_id in &tables {
        let metadata = match provider.load_table(table_id).await {
            Ok(m) => m,
            Err(e) => {
                tracing::warn!("Watch: failed to load table '{}': {}", table_id, e);
                continue;
            }
        };

        let report = engine::check_table(&metadata, config);

        // Compare to previous report.
        let previous = db.get_latest_report(table_id)?;
        if let Some(alert) = detect_changes(&report, previous.as_ref()) {
            db.store_alert(&alert)?;

            // Fire webhook if configured.
            if let Some(ref url) = config.watch.webhook_url
                && let Err(e) = send_webhook(url, &alert).await
            {
                tracing::error!("Watch: webhook failed for '{}': {}", table_id, e);
            }

            alerts.push(alert);
        }

        db.store_report(&report)?;
    }

    Ok(WatchCycleResult {
        tables_checked: tables.len(),
        alerts_fired: alerts.len(),
        alerts,
    })
}

/// Run the watch daemon loop. Blocks until interrupted.
pub async fn run_watch_loop(config: &FrostConfig) -> Result<(), WatchError> {
    let interval_secs = parse_interval(&config.watch.interval)?;
    let db = WatchDb::open(&config.watch.sqlite_path)?;

    tracing::info!(
        "Watch mode started: checking every {}, state at {}",
        config.watch.interval,
        config.watch.sqlite_path
    );

    loop {
        let cycle_start = Utc::now();
        match run_watch_cycle(config, &db).await {
            Ok(result) => {
                tracing::info!(
                    "Watch cycle complete: {} tables checked, {} alerts fired",
                    result.tables_checked,
                    result.alerts_fired
                );
                for alert in &result.alerts {
                    tracing::warn!(
                        "Alert: {} — {} -> {} ({})",
                        alert.table_name,
                        alert.previous_severity,
                        alert.current_severity,
                        alert.message
                    );
                }
            }
            Err(e) => {
                tracing::error!("Watch cycle failed: {}", e);
            }
        }

        let elapsed = (Utc::now() - cycle_start).num_seconds().max(0) as u64;
        let sleep_secs = interval_secs.saturating_sub(elapsed);
        if sleep_secs > 0 {
            tokio::time::sleep(std::time::Duration::from_secs(sleep_secs)).await;
        }
    }
}

// ---------------------------------------------------------------------------
// Change detection + alerting
// ---------------------------------------------------------------------------

/// Compare a new report to the previous one and generate an alert if health changed.
fn detect_changes(current: &HealthReport, previous: Option<&StoredReport>) -> Option<WatchAlert> {
    let current_severity = current.overall.severity.to_string();
    let current_finding_ids: Vec<String> = current
        .findings
        .iter()
        .filter(|f| f.severity != Severity::Pass)
        .map(|f| f.check_id.clone())
        .collect();

    match previous {
        None => {
            // First check — only alert if there are issues.
            if current.overall.severity == Severity::Pass {
                return None;
            }
            Some(WatchAlert {
                table_name: current.table_name.clone(),
                previous_severity: "unknown".to_string(),
                current_severity,
                message: format!(
                    "First check: {} finding(s) detected",
                    current_finding_ids.len()
                ),
                new_findings: current_finding_ids,
                resolved_findings: vec![],
                alerted_at: Utc::now(),
            })
        }
        Some(prev) => {
            // Parse previous report to get finding IDs.
            let prev_finding_ids: Vec<String> =
                serde_json::from_str::<HealthReport>(&prev.report_json)
                    .ok()
                    .map(|r| {
                        r.findings
                            .iter()
                            .filter(|f| f.severity != Severity::Pass)
                            .map(|f| f.check_id.clone())
                            .collect()
                    })
                    .unwrap_or_default();

            let new_findings: Vec<String> = current_finding_ids
                .iter()
                .filter(|id| !prev_finding_ids.contains(id))
                .cloned()
                .collect();

            let resolved_findings: Vec<String> = prev_finding_ids
                .iter()
                .filter(|id| !current_finding_ids.contains(id))
                .cloned()
                .collect();

            // Alert if severity changed or findings changed.
            if new_findings.is_empty()
                && resolved_findings.is_empty()
                && prev.severity == current_severity
            {
                return None;
            }

            let mut parts = Vec::new();
            if !new_findings.is_empty() {
                parts.push(format!("new: {}", new_findings.join(", ")));
            }
            if !resolved_findings.is_empty() {
                parts.push(format!("resolved: {}", resolved_findings.join(", ")));
            }
            if prev.severity != current_severity {
                parts.push(format!("{} -> {}", prev.severity, current_severity));
            }

            Some(WatchAlert {
                table_name: current.table_name.clone(),
                previous_severity: prev.severity.clone(),
                current_severity,
                message: parts.join("; "),
                new_findings,
                resolved_findings,
                alerted_at: Utc::now(),
            })
        }
    }
}

/// Send a webhook alert via HTTP POST.
async fn send_webhook(url: &str, alert: &WatchAlert) -> Result<(), WatchError> {
    let payload = serde_json::json!({
        "text": format!(
            "frost watch alert: *{}* — {}",
            alert.table_name, alert.message
        ),
        "blocks": [
            {
                "type": "section",
                "text": {
                    "type": "mrkdwn",
                    "text": format!(
                        "*frost watch alert*\n*Table:* {}\n*Severity:* {} → {}\n*Details:* {}",
                        alert.table_name,
                        alert.previous_severity,
                        alert.current_severity,
                        alert.message,
                    )
                }
            }
        ]
    });

    let client = reqwest::Client::new();
    let response = client
        .post(url)
        .json(&payload)
        .timeout(std::time::Duration::from_secs(10))
        .send()
        .await
        .map_err(|e| WatchError::Webhook(e.to_string()))?;

    if !response.status().is_success() {
        return Err(WatchError::Webhook(format!(
            "webhook returned status {}",
            response.status()
        )));
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

#[derive(Debug, thiserror::Error)]
pub enum WatchError {
    #[error("watch config error: {0}")]
    Config(String),
    #[error("SQLite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("webhook error: {0}")]
    Webhook(String),
    #[error("watch error: {0}")]
    Other(String),
}
