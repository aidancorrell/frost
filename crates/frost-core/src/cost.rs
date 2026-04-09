//! Cost estimation for Iceberg table health issues.

use crate::config::CostConfig;
use crate::metadata::TableMetadata;
use serde::{Deserialize, Serialize};

/// Cost report for a single table.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CostReport {
    pub table_name: String,
    pub items: Vec<CostItem>,
    pub total_monthly_waste: f64,
    pub currency: String,
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

    // Orphan files: estimate storage waste.
    // We don't have orphan file sizes in metadata alone, so we estimate based on count
    // and average data file size.
    let referenced_count = metadata.data_files.len() + metadata.delete_files.len();
    let orphan_count = metadata
        .all_storage_paths
        .len()
        .saturating_sub(referenced_count);

    if orphan_count > 0 {
        let avg_file_size = if metadata.data_files.is_empty() {
            128 * 1024 * 1024 // assume 128MB if no data files to reference
        } else {
            metadata
                .data_files
                .iter()
                .map(|f| f.file_size_bytes)
                .sum::<u64>()
                / metadata.data_files.len() as u64
        };

        let orphan_bytes = orphan_count as u64 * avg_file_size;
        let orphan_gb = orphan_bytes as f64 / (1024.0 * 1024.0 * 1024.0);
        let monthly = orphan_gb * config.s3_storage_per_gb_month;

        items.push(CostItem {
            category: "orphan_files".to_string(),
            description: format!(
                "{} orphan files (~{:.1} GB estimated) wasting S3 storage",
                orphan_count, orphan_gb,
            ),
            monthly_cost: monthly,
        });
    }

    // Snapshot bloat: excess snapshots retain references to data files,
    // preventing cleanup. Estimate metadata storage cost.
    if metadata.snapshots.len() > 100 {
        let excess = metadata.snapshots.len() - 100;
        // Rough estimate: each excess snapshot keeps ~4KB of metadata
        // plus prevents ~10MB of data file cleanup on average.
        let metadata_waste_gb = (excess as f64 * 4096.0) / (1024.0 * 1024.0 * 1024.0);
        let monthly = metadata_waste_gb * config.s3_storage_per_gb_month;

        items.push(CostItem {
            category: "snapshot_bloat".to_string(),
            description: format!(
                "{} excess snapshots inflating metadata ({:.2} GB)",
                excess, metadata_waste_gb,
            ),
            monthly_cost: monthly,
        });
    }

    // Small files: extra GET requests per query.
    let small_files: Vec<_> = metadata
        .data_files
        .iter()
        .filter(|f| f.file_size_bytes < 8 * 1024 * 1024)
        .collect();
    if !small_files.is_empty() {
        // Estimate: each small file causes 1 extra GET per query.
        // Assume ~100 queries/day against this table.
        let extra_gets_per_day = small_files.len() as f64 * 100.0;
        let extra_gets_per_month = extra_gets_per_day * 30.0;
        let monthly = (extra_gets_per_month / 1000.0) * config.s3_get_request_per_1000;

        items.push(CostItem {
            category: "small_files".to_string(),
            description: format!(
                "{} small files causing ~{:.0} extra S3 GETs/month (at 100 queries/day)",
                small_files.len(),
                extra_gets_per_month,
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
    }
}
