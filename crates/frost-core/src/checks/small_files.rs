use crate::checks::HealthCheck;
use crate::config::Thresholds;
use crate::metadata::TableMetadata;
use crate::report::{Finding, Severity};
use serde_json::json;
use std::collections::HashMap;

/// Small-file detection. Reports counts AND bytes-trapped — bytes is the
/// dimension that drives compaction job sizing, so we surface it explicitly.
pub struct SmallFilesCheck;

impl HealthCheck for SmallFilesCheck {
    fn id(&self) -> &'static str {
        "small_files"
    }

    fn name(&self) -> &'static str {
        "Small Files"
    }

    fn check(&self, metadata: &TableMetadata, thresholds: &Thresholds) -> Finding {
        let threshold = thresholds.small_file_bytes;
        let total_files = metadata.data_files.len() as u64;

        let small_files: Vec<_> = metadata
            .data_files
            .iter()
            .filter(|f| f.file_size_bytes < threshold)
            .collect();

        let small_count = small_files.len() as u64;

        if small_count == 0 {
            return Finding {
                check_id: self.id().to_string(),
                check_name: self.name().to_string(),
                severity: Severity::Pass,
                message: "No small files detected".to_string(),
                impact: String::new(),
                fix_suggestion: None,
                fix_command: None,
                estimated_savings: None,
                details: json!({ "small_file_count": 0, "total_files": total_files }),
            };
        }

        let bytes_trapped: u64 = small_files.iter().map(|f| f.file_size_bytes).sum();
        let total_bytes: u64 = metadata.data_files.iter().map(|f| f.file_size_bytes).sum();
        let bytes_trapped_pct = if total_bytes > 0 {
            (bytes_trapped as f64 / total_bytes as f64) * 100.0
        } else {
            0.0
        };

        let pct = if total_files > 0 {
            (small_count as f64 / total_files as f64) * 100.0
        } else {
            0.0
        };

        // Partition concentration: which partitions hold the most small files?
        let mut by_partition: HashMap<String, u64> = HashMap::new();
        for f in &small_files {
            let key = if f.partition.is_empty() {
                "<unpartitioned>".to_string()
            } else {
                let mut parts: Vec<_> = f.partition.iter().collect();
                parts.sort_by_key(|(k, _)| (*k).clone());
                parts
                    .iter()
                    .map(|(k, v)| format!("{k}={v}"))
                    .collect::<Vec<_>>()
                    .join("/")
            };
            *by_partition.entry(key).or_default() += 1;
        }
        let mut hotspots: Vec<(String, u64)> = by_partition.into_iter().collect();
        hotspots.sort_by_key(|(_, c)| std::cmp::Reverse(*c));
        let top_hotspots: Vec<serde_json::Value> = hotspots
            .iter()
            .take(5)
            .map(|(k, v)| json!({ "partition": k, "small_file_count": v }))
            .collect();

        let severity = if pct > 20.0 || small_count > 500 {
            Severity::Critical
        } else {
            Severity::Warning
        };

        let threshold_mb = threshold / (1024 * 1024);
        let bytes_trapped_mb = bytes_trapped as f64 / (1024.0 * 1024.0);

        Finding {
            check_id: self.id().to_string(),
            check_name: self.name().to_string(),
            severity,
            message: format!(
                "{} files under {}MB ({:.1}% of files, {:.1} MB trapped, {:.1}% of bytes)",
                small_count, threshold_mb, pct, bytes_trapped_mb, bytes_trapped_pct,
            ),
            impact: "Each small file requires a separate S3 GET, a planning task, and \
                     adds query-planning latency proportional to file count. The \
                     'bytes trapped' figure is what compaction has to read+rewrite."
                .to_string(),
            fix_suggestion: Some(
                "Run rewrite_data_files with binpack strategy to compact small files".to_string(),
            ),
            fix_command: Some(format!(
                "CALL catalog.system.rewrite_data_files(table => '{}', strategy => 'binpack')",
                metadata.table_name
            )),
            estimated_savings: Some(format!(
                "~{:.0}% faster scans after compaction, ~{} fewer S3 GET requests per query",
                (pct * 0.8).min(50.0),
                small_count,
            )),
            details: json!({
                "small_file_count": small_count,
                "total_files": total_files,
                "small_file_percentage": format!("{:.1}", pct),
                "bytes_trapped": bytes_trapped,
                "bytes_trapped_pct": format!("{:.1}", bytes_trapped_pct),
                "threshold_bytes": threshold,
                "top_partitions": top_hotspots,
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::metadata::*;
    use crate::test_helpers::make_test_metadata;

    #[test]
    fn healthy_table_passes() {
        let mut meta = make_test_metadata();
        meta.data_files = (0..10)
            .map(|i| DataFile {
                file_path: format!("s3://bucket/data/part-{i}.parquet"),
                file_size_bytes: 100 * 1024 * 1024,
                record_count: 1_000_000,
                partition: Default::default(),
                file_format: FileFormat::Parquet,
                ..Default::default()
            })
            .collect();

        let finding = SmallFilesCheck.check(&meta, &Thresholds::default());
        assert_eq!(finding.severity, Severity::Pass);
    }

    #[test]
    fn detects_small_files_and_reports_bytes_trapped() {
        let mut meta = make_test_metadata();
        meta.data_files = (0..200)
            .map(|i| DataFile {
                file_path: format!("s3://bucket/data/part-{i}.parquet"),
                file_size_bytes: if i < 30 {
                    1024 * 100
                } else {
                    100 * 1024 * 1024
                },
                record_count: if i < 30 { 100 } else { 1_000_000 },
                partition: Default::default(),
                file_format: FileFormat::Parquet,
                ..Default::default()
            })
            .collect();

        let finding = SmallFilesCheck.check(&meta, &Thresholds::default());
        assert_eq!(finding.severity, Severity::Warning);
        assert!(finding.message.contains("30 files"));
        assert!(finding.message.contains("trapped"));
        assert_eq!(finding.details["small_file_count"], 30);
        assert!(finding.details["bytes_trapped"].as_u64().unwrap() > 0);
    }

    #[test]
    fn critical_when_many_small_files() {
        let mut meta = make_test_metadata();
        meta.data_files = (0..700)
            .map(|i| DataFile {
                file_path: format!("s3://bucket/data/part-{i}.parquet"),
                file_size_bytes: if i < 600 { 1024 } else { 100 * 1024 * 1024 },
                record_count: 100,
                partition: Default::default(),
                file_format: FileFormat::Parquet,
                ..Default::default()
            })
            .collect();

        let finding = SmallFilesCheck.check(&meta, &Thresholds::default());
        assert_eq!(finding.severity, Severity::Critical);
    }
}
