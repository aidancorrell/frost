//! Tests for MCP tool handlers.
//!
//! These tests invoke the public tool handler methods directly (not over a transport)
//! to verify they produce correct structured responses.

use frost_core::config::{CatalogConfig, FrostConfig};
use frost_mcp::server::FrostServer;

mod fixture_helpers;

// --- check_table ---

#[tokio::test]
async fn check_table_returns_findings() {
    let tmp = tempfile::TempDir::new().unwrap();
    fixture_helpers::create_small_files_table(tmp.path());

    let config = FrostConfig {
        catalog: CatalogConfig::Filesystem {
            warehouse: tmp.path().to_string_lossy().to_string(),
        },
        ..Default::default()
    };

    let server = FrostServer::new(config);
    let result = server
        .run_check_table(frost_mcp::tools::CheckTableParams {
            table: "test_ns.small_files_table".to_string(),
            checks: None,
        })
        .await;

    let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();
    assert!(parsed["findings"].is_array());
    assert!(!parsed["findings"].as_array().unwrap().is_empty());
    assert!(parsed["table_name"].as_str().unwrap().contains("small_files"));
}

#[tokio::test]
async fn check_table_with_filter() {
    let tmp = tempfile::TempDir::new().unwrap();
    fixture_helpers::create_small_files_table(tmp.path());

    let config = FrostConfig {
        catalog: CatalogConfig::Filesystem {
            warehouse: tmp.path().to_string_lossy().to_string(),
        },
        ..Default::default()
    };

    let server = FrostServer::new(config);
    let result = server
        .run_check_table(frost_mcp::tools::CheckTableParams {
            table: "test_ns.small_files_table".to_string(),
            checks: Some(vec!["small_files".to_string()]),
        })
        .await;

    let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();
    let findings = parsed["findings"].as_array().unwrap();
    assert_eq!(findings.len(), 1);
    assert_eq!(findings[0]["check_id"], "small_files");
}

#[tokio::test]
async fn check_table_error_on_missing() {
    let tmp = tempfile::TempDir::new().unwrap();

    let config = FrostConfig {
        catalog: CatalogConfig::Filesystem {
            warehouse: tmp.path().to_string_lossy().to_string(),
        },
        ..Default::default()
    };

    let server = FrostServer::new(config);
    let result = server
        .run_check_table(frost_mcp::tools::CheckTableParams {
            table: "nonexistent.table".to_string(),
            checks: None,
        })
        .await;

    let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();
    assert!(parsed["error"].is_string());
}

// --- check_catalog ---

#[tokio::test]
async fn check_catalog_lists_all_tables() {
    let tmp = tempfile::TempDir::new().unwrap();
    fixture_helpers::create_healthy_table(tmp.path());
    fixture_helpers::create_small_files_table(tmp.path());

    let config = FrostConfig {
        catalog: CatalogConfig::Filesystem {
            warehouse: tmp.path().to_string_lossy().to_string(),
        },
        ..Default::default()
    };

    let server = FrostServer::new(config);
    let result = server
        .run_check_catalog(frost_mcp::tools::CheckCatalogParams {
            namespace: None,
        })
        .await;

    let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();
    assert_eq!(parsed["tables_checked"].as_u64().unwrap(), 2);
    assert!(parsed["table_reports"].is_array());
}

// --- get_fix ---

#[tokio::test]
async fn get_fix_returns_command() {
    let tmp = tempfile::TempDir::new().unwrap();
    fixture_helpers::create_small_files_table(tmp.path());

    let config = FrostConfig {
        catalog: CatalogConfig::Filesystem {
            warehouse: tmp.path().to_string_lossy().to_string(),
        },
        ..Default::default()
    };

    let server = FrostServer::new(config);
    let result = server
        .run_get_fix(frost_mcp::tools::GetFixParams {
            table: "test_ns.small_files_table".to_string(),
            finding_id: "small_files".to_string(),
        })
        .await;

    let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();
    assert!(parsed["command"].as_str().unwrap().contains("rewrite_data_files"));
}

#[tokio::test]
async fn get_fix_unknown_finding() {
    let tmp = tempfile::TempDir::new().unwrap();
    fixture_helpers::create_healthy_table(tmp.path());

    let config = FrostConfig {
        catalog: CatalogConfig::Filesystem {
            warehouse: tmp.path().to_string_lossy().to_string(),
        },
        ..Default::default()
    };

    let server = FrostServer::new(config);
    let result = server
        .run_get_fix(frost_mcp::tools::GetFixParams {
            table: "test_ns.healthy_table".to_string(),
            finding_id: "nonexistent".to_string(),
        })
        .await;

    let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();
    assert!(parsed["error"].is_string());
    assert!(parsed["available_findings"].is_array());
}

// --- get_cost_report ---

#[tokio::test]
async fn get_cost_report_returns_items() {
    let tmp = tempfile::TempDir::new().unwrap();
    fixture_helpers::create_small_files_table(tmp.path());

    let config = FrostConfig {
        catalog: CatalogConfig::Filesystem {
            warehouse: tmp.path().to_string_lossy().to_string(),
        },
        ..Default::default()
    };

    let server = FrostServer::new(config);
    let result = server
        .run_get_cost_report(frost_mcp::tools::GetCostReportParams {
            table: "test_ns.small_files_table".to_string(),
        })
        .await;

    let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();
    assert!(parsed["items"].is_array());
    assert!(parsed["total_monthly_waste"].is_number());
}

// --- watch_status ---

#[tokio::test]
async fn watch_status_returns_not_running() {
    let config = FrostConfig::default();
    let server = FrostServer::new(config);
    let result = server
        .run_watch_status(frost_mcp::tools::WatchStatusParams {
            table: None,
        })
        .await;

    let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();
    assert_eq!(parsed["status"], "not_running");
}
