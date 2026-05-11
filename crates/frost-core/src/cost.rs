//! Cost estimation for Iceberg table health issues.
//!
//! Models four cost categories:
//!  1. Storage waste from orphan files (sized via avg data file size).
//!  2. Storage waste from retained snapshots (real metadata size, scaled).
//!  3. Planning-time S3 GETs from small files and many manifests.
//!  4. Scan-time compute overhead from delete files (merge-on-read).
//!
//! Every assumption (queries-per-day, compute cost, avg scan bytes) is
//! configurable in `[cost]` so teams can tune to their workload.

use crate::config::CostConfig;
use crate::metadata::{DeleteType, TableMetadata};
use serde::{Deserialize, Serialize};

const BYTES_PER_GB: f64 = 1024.0 * 1024.0 * 1024.0;
const DAYS_PER_MONTH: f64 = 30.0;
const SECONDS_PER_HOUR: f64 = 3600.0;

/// Cost report for a single table.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CostReport {
    pub table_name: String,
    pub items: Vec<CostItem>,
    pub total_monthly_waste: f64,
    pub currency: String,
    /// Assumptions used to compute this report. Surfaced so the reader
    /// can sanity-check the dollar figure.
    pub assumptions: CostAssumptions,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CostAssumptions {
    pub queries_per_day: f64,
    pub avg_bytes_scanned_per_query: u64,
    pub compute_cost_per_cpu_hour: f64,
    pub s3_storage_per_gb_month: f64,
    pub s3_get_request_per_1000: f64,
}

/// A single cost waste item.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CostItem {
    pub category: String,
    pub description: String,
    pub monthly_cost: f64,
}

/// Estimate monthly cost waste for a table based on its metadata.
pub fn estimate_cost(metadata: &TableMetadata, config: &CostConfig) -> CostReport {
    let mut items = Vec::new();

    // 1. Orphan files.
    let referenced_count = metadata.data_files.len() + metadata.delete_files.len();
    let orphan_count = metadata
        .all_storage_paths
        .len()
        .saturating_sub(referenced_count);

    if orphan_count > 0 {
        let avg_file_size = if metadata.data_files.is_empty() {
            128 * 1024 * 1024
        } else {
            metadata
                .data_files
                .iter()
                .map(|f| f.file_size_bytes)
                .sum::<u64>()
                / metadata.data_files.len() as u64
        };
        let orphan_gb = (orphan_count as u64 * avg_file_size) as f64 / BYTES_PER_GB;
        let monthly = orphan_gb * config.s3_storage_per_gb_month;
        items.push(CostItem {
            category: "orphan_files".to_string(),
            description: format!(
                "{} orphan files (~{:.1} GB at avg {} MB/file) wasting S3 storage",
                orphan_count,
                orphan_gb,
                avg_file_size / (1024 * 1024),
            ),
            monthly_cost: monthly,
        });
    }

    // 2. Snapshot bloat — uses REAL metadata size, not magic numbers.
    // Scale: each excess snapshot's pro-rata share of metadata storage.
    if metadata.snapshots.len() > 100 && metadata.metadata_size_bytes > 0 {
        let excess = metadata.snapshots.len() - 100;
        let bloat_fraction = excess as f64 / metadata.snapshots.len() as f64;
        let bloat_bytes = metadata.metadata_size_bytes as f64 * bloat_fraction;
        let bloat_gb = bloat_bytes / BYTES_PER_GB;
        let monthly = bloat_gb * config.s3_storage_per_gb_month;
        items.push(CostItem {
            category: "snapshot_bloat".to_string(),
            description: format!(
                "{} excess snapshots holding ~{:.2} MB of metadata storage open",
                excess,
                bloat_bytes / (1024.0 * 1024.0),
            ),
            monthly_cost: monthly,
        });
    }

    // 3. Small files: planning-time S3 GETs.
    let small_threshold = 8 * 1024 * 1024;
    let small_count = metadata
        .data_files
        .iter()
        .filter(|f| f.file_size_bytes < small_threshold)
        .count();
    if small_count > 0 {
        let extra_gets_per_month = small_count as f64 * config.queries_per_day * DAYS_PER_MONTH;
        let monthly = (extra_gets_per_month / 1000.0) * config.s3_get_request_per_1000;
        items.push(CostItem {
            category: "small_files".to_string(),
            description: format!(
                "{} small files causing ~{:.0} extra S3 GETs/month at {:.0} queries/day",
                small_count, extra_gets_per_month, config.queries_per_day,
            ),
            monthly_cost: monthly,
        });
    }

    // 4. Manifest count: planning-time GETs scale with manifest count too.
    if metadata.manifest_stats.manifest_count > 0 {
        let manifest_gets_per_month =
            metadata.manifest_stats.manifest_count as f64 * config.queries_per_day * DAYS_PER_MONTH;
        let monthly = (manifest_gets_per_month / 1000.0) * config.s3_get_request_per_1000;
        items.push(CostItem {
            category: "manifest_planning_gets".to_string(),
            description: format!(
                "{} manifests fetched per query (median {:.1} KB, max {:.1} KB)",
                metadata.manifest_stats.manifest_count,
                metadata.manifest_stats.median_manifest_bytes as f64 / 1024.0,
                metadata.manifest_stats.max_manifest_bytes as f64 / 1024.0,
            ),
            monthly_cost: monthly,
        });
    }

    // 5. Delete-file scan-time overhead. Equality deletes are ~5× costlier
    // than position deletes; we model both as a percentage of normal scan
    // CPU time. Numbers are coarse but at least *derived from* something.
    let position_deletes = metadata
        .delete_files
        .iter()
        .filter(|f| f.delete_type == DeleteType::PositionDelete)
        .count() as f64;
    let equality_deletes = metadata
        .delete_files
        .iter()
        .filter(|f| f.delete_type == DeleteType::EqualityDelete)
        .count() as f64;

    if position_deletes > 0.0 || equality_deletes > 0.0 {
        // Each position delete adds ~0.1% scan-time overhead; each equality
        // delete adds ~0.5%. Cap at 100% (a fully-shadowed table).
        let scan_overhead_pct = (position_deletes * 0.001 + equality_deletes * 0.005).min(1.0);

        // Approximate CPU-seconds per query: bytes_scanned / 100 MB/s (a
        // conservative scan throughput for Parquet on Spark).
        let scan_throughput_bytes_per_sec = 100.0 * 1024.0 * 1024.0;
        let cpu_secs_per_query =
            config.avg_bytes_scanned_per_query as f64 / scan_throughput_bytes_per_sec;
        let extra_cpu_secs_per_query = cpu_secs_per_query * scan_overhead_pct;
        let extra_cpu_hours_per_month =
            extra_cpu_secs_per_query * config.queries_per_day * DAYS_PER_MONTH / SECONDS_PER_HOUR;
        let monthly = extra_cpu_hours_per_month * config.compute_cost_per_cpu_hour;

        items.push(CostItem {
            category: "delete_merge_overhead".to_string(),
            description: format!(
                "{} position + {} equality deletes adding ~{:.1}% scan-time overhead per query",
                position_deletes as u64,
                equality_deletes as u64,
                scan_overhead_pct * 100.0,
            ),
            monthly_cost: monthly,
        });
    }

    let total = items.iter().map(|i| i.monthly_cost).sum();

    CostReport {
        table_name: metadata.table_name.clone(),
        items,
        total_monthly_waste: total,
        currency: "USD".to_string(),
        assumptions: CostAssumptions {
            queries_per_day: config.queries_per_day,
            avg_bytes_scanned_per_query: config.avg_bytes_scanned_per_query,
            compute_cost_per_cpu_hour: config.compute_cost_per_cpu_hour,
            s3_storage_per_gb_month: config.s3_storage_per_gb_month,
            s3_get_request_per_1000: config.s3_get_request_per_1000,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::metadata::{DataFile, DeleteFile, DeleteType, FileFormat};
    use crate::test_helpers::make_test_metadata;

    #[test]
    fn empty_table_has_no_waste() {
        let meta = make_test_metadata();
        let report = estimate_cost(&meta, &CostConfig::default());
        assert!(report.items.is_empty());
        assert_eq!(report.total_monthly_waste, 0.0);
    }

    #[test]
    fn delete_files_drive_compute_cost() {
        let mut meta = make_test_metadata();
        meta.data_files = (0..10)
            .map(|i| DataFile {
                file_path: format!("d{i}"),
                file_size_bytes: 100 * 1024 * 1024,
                record_count: 1_000_000,
                file_format: FileFormat::Parquet,
                ..Default::default()
            })
            .collect();
        meta.delete_files = (0..50)
            .map(|i| DeleteFile {
                file_path: format!("eq{i}"),
                file_size_bytes: 1024,
                record_count: 100,
                delete_type: DeleteType::EqualityDelete,
                equality_ids: vec![1],
            })
            .collect();
        let report = estimate_cost(&meta, &CostConfig::default());
        let merge = report
            .items
            .iter()
            .find(|i| i.category == "delete_merge_overhead")
            .expect("delete_merge_overhead should be reported");
        assert!(merge.monthly_cost > 0.0);
    }

    #[test]
    fn assumptions_are_surfaced() {
        let meta = make_test_metadata();
        let report = estimate_cost(&meta, &CostConfig::default());
        assert_eq!(report.assumptions.queries_per_day, 100.0);
        assert_eq!(report.assumptions.compute_cost_per_cpu_hour, 0.05);
    }
}
