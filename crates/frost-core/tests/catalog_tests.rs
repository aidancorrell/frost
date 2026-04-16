//! Integration tests for the filesystem catalog + health checks.
//!
//! These tests create real Iceberg table layouts on disk (metadata JSON +
//! Avro manifest files), load them through the FilesystemCatalog, and verify
//! that health checks produce correct findings.

use frost_core::catalog::{CatalogProvider, FilesystemCatalog};
use frost_core::config::FrostConfig;
use frost_core::engine;
use frost_core::report::Severity;
use tempfile::TempDir;

// Import the fixture generators.
// These are cfg(test) in frost-core, so we re-implement minimal versions here
// or use a shared test-support approach. Since the fixtures module is internal
// to frost-core, we'll create tables using the same approach inline.
mod fixture_helpers;

#[tokio::test]
async fn healthy_table_all_checks_pass() {
    let tmp = TempDir::new().unwrap();
    fixture_helpers::create_healthy_table(tmp.path());

    let catalog = FilesystemCatalog::new(tmp.path());
    let meta = catalog.load_table("test_ns.healthy_table").await.unwrap();

    let config = FrostConfig::default();
    let report = engine::check_table(&meta, &config);

    // Healthy table should have all passes (except possibly freshness depending on timing).
    for finding in &report.findings {
        if finding.check_id != "freshness" {
            assert_eq!(
                finding.severity,
                Severity::Pass,
                "Check '{}' should pass on healthy table, got {:?}: {}",
                finding.check_id,
                finding.severity,
                finding.message,
            );
        }
    }
}

#[tokio::test]
async fn small_files_detected() {
    let tmp = TempDir::new().unwrap();
    fixture_helpers::create_small_files_table(tmp.path());

    let catalog = FilesystemCatalog::new(tmp.path());
    let meta = catalog
        .load_table("test_ns.small_files_table")
        .await
        .unwrap();

    let config = FrostConfig::default();
    let report = engine::check_table(&meta, &config);

    let finding = report
        .findings
        .iter()
        .find(|f| f.check_id == "small_files")
        .unwrap();
    assert!(
        finding.severity >= Severity::Warning,
        "Small files should be flagged: {:?} - {}",
        finding.severity,
        finding.message,
    );
    assert!(finding.fix_command.is_some());
}

#[tokio::test]
async fn snapshot_bloat_detected() {
    let tmp = TempDir::new().unwrap();
    fixture_helpers::create_snapshot_bloat_table(tmp.path());

    let catalog = FilesystemCatalog::new(tmp.path());
    let meta = catalog
        .load_table("test_ns.snapshot_bloat_table")
        .await
        .unwrap();

    let config = FrostConfig::default();
    let report = engine::check_table(&meta, &config);

    let finding = report
        .findings
        .iter()
        .find(|f| f.check_id == "snapshot_bloat")
        .unwrap();
    assert!(
        finding.severity >= Severity::Warning,
        "Snapshot bloat should be flagged: {:?} - {}",
        finding.severity,
        finding.message,
    );
    assert!(finding.fix_command.is_some());
}

#[tokio::test]
async fn orphan_files_detected() {
    let tmp = TempDir::new().unwrap();
    fixture_helpers::create_orphan_files_table(tmp.path());

    let catalog = FilesystemCatalog::new(tmp.path());
    let meta = catalog
        .load_table("test_ns.orphan_files_table")
        .await
        .unwrap();

    let config = FrostConfig::default();
    let report = engine::check_table(&meta, &config);

    let finding = report
        .findings
        .iter()
        .find(|f| f.check_id == "orphan_files")
        .unwrap();
    assert!(
        finding.severity >= Severity::Warning,
        "Orphan files should be flagged: {:?} - {}",
        finding.severity,
        finding.message,
    );
    assert!(finding.message.contains("15 files") || finding.message.contains("orphan"));
}

#[tokio::test]
async fn schema_drift_detected() {
    let tmp = TempDir::new().unwrap();
    fixture_helpers::create_schema_drift_table(tmp.path());

    let catalog = FilesystemCatalog::new(tmp.path());
    let meta = catalog
        .load_table("test_ns.schema_drift_table")
        .await
        .unwrap();

    let config = FrostConfig::default();
    let report = engine::check_table(&meta, &config);

    let finding = report
        .findings
        .iter()
        .find(|f| f.check_id == "schema_history")
        .unwrap();
    assert!(
        finding.severity >= Severity::Warning,
        "Schema drift should be flagged: {:?} - {}",
        finding.severity,
        finding.message,
    );
    // Should detect: email dropped, age type changed (int -> string).
    assert!(finding.message.contains("breaking"));
}

#[tokio::test]
async fn list_tables_finds_all() {
    let tmp = TempDir::new().unwrap();
    fixture_helpers::create_healthy_table(tmp.path());
    fixture_helpers::create_small_files_table(tmp.path());
    fixture_helpers::create_orphan_files_table(tmp.path());

    let catalog = FilesystemCatalog::new(tmp.path());
    let tables = catalog.list_tables(None).await.unwrap();

    assert_eq!(tables.len(), 3);
    assert!(tables.contains(&"test_ns.healthy_table".to_string()));
    assert!(tables.contains(&"test_ns.small_files_table".to_string()));
    assert!(tables.contains(&"test_ns.orphan_files_table".to_string()));
}

#[tokio::test]
async fn list_tables_filters_by_namespace() {
    let tmp = TempDir::new().unwrap();
    fixture_helpers::create_healthy_table(tmp.path());

    let catalog = FilesystemCatalog::new(tmp.path());

    let tables = catalog.list_tables(Some("test_ns")).await.unwrap();
    assert_eq!(tables.len(), 1);

    let tables = catalog.list_tables(Some("nonexistent")).await.unwrap();
    assert!(tables.is_empty());
}

#[tokio::test]
async fn cost_report_for_table_with_issues() {
    let tmp = TempDir::new().unwrap();
    fixture_helpers::create_small_files_table(tmp.path());

    let catalog = FilesystemCatalog::new(tmp.path());
    let meta = catalog
        .load_table("test_ns.small_files_table")
        .await
        .unwrap();

    let config = FrostConfig::default();
    let cost_report = frost_core::cost::estimate_cost(&meta, &config.cost);

    // Should have at least one cost item for small files.
    assert!(
        !cost_report.items.is_empty(),
        "Cost report should have items for table with issues"
    );
}

#[tokio::test]
async fn fix_generation_for_findings() {
    let tmp = TempDir::new().unwrap();
    fixture_helpers::create_small_files_table(tmp.path());

    let catalog = FilesystemCatalog::new(tmp.path());
    let meta = catalog
        .load_table("test_ns.small_files_table")
        .await
        .unwrap();

    // Test fix generation for small_files.
    let fix = frost_core::fix::generate_fix(&meta, "small_files");
    assert!(fix.is_some());
    let fix = fix.unwrap();
    assert!(fix.command.contains("rewrite_data_files"));
    assert!(fix.command.contains(&meta.table_name));

    // Test fix generation for snapshot_bloat.
    let fix = frost_core::fix::generate_fix(&meta, "snapshot_bloat");
    assert!(fix.is_some());
    assert!(fix.unwrap().command.contains("expire_snapshots"));

    // Test fix generation for unknown finding.
    let fix = frost_core::fix::generate_fix(&meta, "nonexistent");
    assert!(fix.is_none());
}

#[tokio::test]
async fn json_output_is_valid() {
    let tmp = TempDir::new().unwrap();
    fixture_helpers::create_healthy_table(tmp.path());

    let catalog = FilesystemCatalog::new(tmp.path());
    let meta = catalog.load_table("test_ns.healthy_table").await.unwrap();

    let config = FrostConfig::default();
    let report = engine::check_table(&meta, &config);

    // Verify the report serializes to valid JSON.
    let json = serde_json::to_string_pretty(&report).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();

    assert!(parsed["table_name"].is_string());
    assert!(parsed["findings"].is_array());
    assert!(parsed["overall"]["severity"].is_string());
}
