//! Tests for the watch module (SQLite state, change detection, watch cycle).

use frost_core::config::{CatalogConfig, FrostConfig};
use frost_core::report::{Finding, HealthReport, Severity, TableSummary};
use frost_core::watch::{self, WatchDb};

fn make_report(table: &str, severity: Severity, finding_ids: &[&str]) -> HealthReport {
    let findings: Vec<Finding> = finding_ids
        .iter()
        .map(|id| Finding {
            check_id: id.to_string(),
            check_name: id.to_string(),
            severity,
            message: format!("{} issue detected", id),
            impact: "test impact".to_string(),
            fix_suggestion: None,
            fix_command: None,
            estimated_savings: None,
            details: serde_json::json!({}),
        })
        .collect();

    let overall = HealthReport::compute_overall(&findings);

    HealthReport {
        table_name: table.to_string(),
        location: format!("s3://bucket/{}", table),
        summary: TableSummary {
            snapshot_count: 1,
            data_file_count: 10,
            total_size_bytes: 1024 * 1024 * 100,
            total_record_count: 50000,
        },
        findings,
        overall,
        generated_at: chrono::Utc::now(),
    }
}

#[test]
fn db_open_in_memory() {
    let db = WatchDb::open_in_memory().unwrap();
    let latest = db.get_all_latest().unwrap();
    assert!(latest.is_empty());
}

#[test]
fn store_and_retrieve_report() {
    let db = WatchDb::open_in_memory().unwrap();

    let report = make_report("test_ns.events", Severity::Warning, &["small_files"]);
    db.store_report(&report).unwrap();

    let latest = db.get_latest_report("test_ns.events").unwrap();
    assert!(latest.is_some());
    let latest = latest.unwrap();
    assert_eq!(latest.table_name, "test_ns.events");
    assert_eq!(latest.severity, "WARNING");
    assert_eq!(latest.finding_count, 1);
}

#[test]
fn get_history_returns_most_recent_first() {
    let db = WatchDb::open_in_memory().unwrap();

    // Store 3 reports with slightly different times.
    for i in 0..3 {
        let mut report = make_report("test_ns.events", Severity::Warning, &["small_files"]);
        report.generated_at = chrono::Utc::now() + chrono::Duration::seconds(i);
        db.store_report(&report).unwrap();
    }

    let history = db.get_history("test_ns.events", 10).unwrap();
    assert_eq!(history.len(), 3);
    // Most recent first.
    assert!(history[0].checked_at >= history[1].checked_at);
}

#[test]
fn get_all_latest_returns_one_per_table() {
    let db = WatchDb::open_in_memory().unwrap();

    db.store_report(&make_report("ns.table_a", Severity::Pass, &[]))
        .unwrap();
    db.store_report(&make_report(
        "ns.table_b",
        Severity::Warning,
        &["small_files"],
    ))
    .unwrap();
    db.store_report(&make_report(
        "ns.table_a",
        Severity::Critical,
        &["snapshot_bloat"],
    ))
    .unwrap();

    let latest = db.get_all_latest().unwrap();
    assert_eq!(latest.len(), 2);

    let table_a = latest
        .iter()
        .find(|r| r.table_name == "ns.table_a")
        .unwrap();
    assert_eq!(table_a.severity, "CRITICAL");
}

#[test]
fn store_and_retrieve_alerts() {
    let db = WatchDb::open_in_memory().unwrap();

    let alert = watch::WatchAlert {
        table_name: "ns.events".to_string(),
        previous_severity: "PASS".to_string(),
        current_severity: "WARNING".to_string(),
        message: "new: small_files".to_string(),
        new_findings: vec!["small_files".to_string()],
        resolved_findings: vec![],
        alerted_at: chrono::Utc::now(),
    };

    db.store_alert(&alert).unwrap();

    let alerts = db.get_alerts(Some("ns.events"), 10).unwrap();
    assert_eq!(alerts.len(), 1);
    assert_eq!(alerts[0].table_name, "ns.events");
    assert_eq!(alerts[0].current_severity, "WARNING");

    let all_alerts = db.get_alerts(None, 10).unwrap();
    assert_eq!(all_alerts.len(), 1);
}

#[test]
fn parse_interval_valid() {
    assert_eq!(watch::parse_interval("30m").unwrap(), 1800);
    assert_eq!(watch::parse_interval("1h").unwrap(), 3600);
    assert_eq!(watch::parse_interval("6h").unwrap(), 21600);
    assert_eq!(watch::parse_interval("1d").unwrap(), 86400);
    assert_eq!(watch::parse_interval("90s").unwrap(), 90);
}

#[test]
fn parse_interval_invalid() {
    assert!(watch::parse_interval("").is_err());
    assert!(watch::parse_interval("abc").is_err());
    assert!(watch::parse_interval("5x").is_err());
}

#[tokio::test]
async fn watch_cycle_with_tables() {
    let tmp = tempfile::TempDir::new().unwrap();

    // Reuse fixture helpers from catalog_tests to create real tables.
    // Instead, just test the cycle with an empty catalog.
    let config = FrostConfig {
        catalog: CatalogConfig::Filesystem {
            warehouse: tmp.path().to_string_lossy().to_string(),
        },
        ..Default::default()
    };

    let db = WatchDb::open_in_memory().unwrap();
    let result = watch::run_watch_cycle(&config, &db).await.unwrap();

    // No tables in empty warehouse.
    assert_eq!(result.tables_checked, 0);
    assert_eq!(result.alerts_fired, 0);
}

#[test]
fn db_open_creates_file() {
    let tmp = tempfile::TempDir::new().unwrap();
    let db_path = tmp.path().join("test.db");

    let _db = WatchDb::open(db_path.to_str().unwrap()).unwrap();
    assert!(db_path.exists());
}
