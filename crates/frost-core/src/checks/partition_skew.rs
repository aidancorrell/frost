use crate::checks::HealthCheck;
use crate::config::Thresholds;
use crate::metadata::TableMetadata;
use crate::report::{Finding, Severity};
use serde_json::json;
use std::collections::HashMap;

/// Partition skew, measured by bytes and rows — not file counts.
///
/// File-count skew misses the real problem (a partition with one 100 GB file
/// is not "balanced" with neighbors holding one 1 KB file). We report the
/// max/median ratio for both bytes and rows, plus p95/p99 distribution
/// percentiles, and flag whichever dimension trips the threshold.
pub struct PartitionSkewCheck;

impl HealthCheck for PartitionSkewCheck {
    fn id(&self) -> &'static str {
        "partition_skew"
    }

    fn name(&self) -> &'static str {
        "Partition Skew"
    }

    fn check(&self, metadata: &TableMetadata, thresholds: &Thresholds) -> Finding {
        if metadata.partition_spec.fields.is_empty() {
            return Finding {
                check_id: self.id().to_string(),
                check_name: self.name().to_string(),
                severity: Severity::Pass,
                message: "Unpartitioned table — skew check not applicable".to_string(),
                impact: String::new(),
                fix_suggestion: None,
                fix_command: None,
                estimated_savings: None,
                details: json!({ "partitioned": false }),
            };
        }

        // Aggregate per-partition: file count, total bytes, total rows.
        let mut by_partition: HashMap<String, PartitionAgg> = HashMap::new();
        for file in &metadata.data_files {
            let key = partition_key(&file.partition);
            let entry = by_partition.entry(key).or_default();
            entry.file_count += 1;
            entry.total_bytes += file.file_size_bytes;
            entry.total_rows += file.record_count;
        }

        if by_partition.is_empty() {
            return Finding {
                check_id: self.id().to_string(),
                check_name: self.name().to_string(),
                severity: Severity::Pass,
                message: "No data files to analyze for partition skew".to_string(),
                impact: String::new(),
                fix_suggestion: None,
                fix_command: None,
                estimated_savings: None,
                details: json!({ "partitioned": true, "partition_count": 0 }),
            };
        }

        let bytes_dist = Distribution::from_iter(by_partition.values().map(|a| a.total_bytes));
        let rows_dist = Distribution::from_iter(by_partition.values().map(|a| a.total_rows));
        let files_dist = Distribution::from_iter(by_partition.values().map(|a| a.file_count));

        // Hot partition: the one driving the worst byte ratio.
        let (max_partition, _) = by_partition
            .iter()
            .max_by_key(|(_, a)| a.total_bytes)
            .map(|(k, v)| (k.clone(), v.total_bytes))
            .unwrap_or_default();

        let bytes_ratio = bytes_dist.ratio_max_over_median();
        let rows_ratio = rows_dist.ratio_max_over_median();
        let files_ratio = files_dist.ratio_max_over_median();

        // Use the worst-offending dimension to decide severity.
        let worst_ratio = bytes_ratio.max(rows_ratio).max(files_ratio);

        if worst_ratio <= thresholds.partition_skew_ratio {
            return Finding {
                check_id: self.id().to_string(),
                check_name: self.name().to_string(),
                severity: Severity::Pass,
                message: format!(
                    "Partition distribution looks balanced (max/median: bytes {:.1}x, rows {:.1}x, files {:.1}x)",
                    bytes_ratio, rows_ratio, files_ratio,
                ),
                impact: String::new(),
                fix_suggestion: None,
                fix_command: None,
                estimated_savings: None,
                details: json!({
                    "partition_count": by_partition.len(),
                    "bytes": bytes_dist.to_json(),
                    "rows": rows_dist.to_json(),
                    "files": files_dist.to_json(),
                    "max_partition": max_partition,
                }),
            };
        }

        let severity = if worst_ratio > thresholds.partition_skew_ratio * 5.0 {
            Severity::Critical
        } else {
            Severity::Warning
        };

        let driving_dimension = if bytes_ratio >= rows_ratio && bytes_ratio >= files_ratio {
            "bytes"
        } else if rows_ratio >= files_ratio {
            "rows"
        } else {
            "files"
        };

        Finding {
            check_id: self.id().to_string(),
            check_name: self.name().to_string(),
            severity,
            message: format!(
                "Partition '{}' is {:.1}x median by {} (max/median: bytes {:.1}x, rows {:.1}x, files {:.1}x)",
                max_partition, worst_ratio, driving_dimension, bytes_ratio, rows_ratio, files_ratio,
            ),
            impact: "Hot partitions cause Spark task stragglers — one executor does \
                     disproportionate work, turning a fast job into a slow one. Byte \
                     and row skew matter more than file-count skew."
                .to_string(),
            fix_suggestion: Some(
                "Repartition with a higher-cardinality transform, or rewrite with \
                 sort/zorder strategy to redistribute data more evenly."
                    .to_string(),
            ),
            fix_command: Some(format!(
                "CALL catalog.system.rewrite_data_files(table => '{}', strategy => 'sort', \
                 sort_order => 'zorder({})')",
                metadata.table_name,
                metadata
                    .partition_spec
                    .fields
                    .iter()
                    .map(|f| f.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", "),
            )),
            estimated_savings: Some(format!(
                "Reduces straggler time on partition '{}'",
                max_partition,
            )),
            details: json!({
                "partition_count": by_partition.len(),
                "driving_dimension": driving_dimension,
                "bytes": bytes_dist.to_json(),
                "rows": rows_dist.to_json(),
                "files": files_dist.to_json(),
                "max_partition": max_partition,
            }),
        }
    }
}

#[derive(Default)]
struct PartitionAgg {
    file_count: u64,
    total_bytes: u64,
    total_rows: u64,
}

#[derive(Debug)]
struct Distribution {
    median: u64,
    p95: u64,
    p99: u64,
    max: u64,
    sum: u64,
    count: u64,
}

impl Distribution {
    fn from_iter<I: IntoIterator<Item = u64>>(iter: I) -> Self {
        let mut values: Vec<u64> = iter.into_iter().collect();
        values.sort_unstable();
        if values.is_empty() {
            return Self {
                median: 0,
                p95: 0,
                p99: 0,
                max: 0,
                sum: 0,
                count: 0,
            };
        }
        let n = values.len();
        let percentile = |p: f64| -> u64 {
            let idx = ((p / 100.0) * (n as f64 - 1.0)).round() as usize;
            values[idx.min(n - 1)]
        };
        Self {
            median: percentile(50.0),
            p95: percentile(95.0),
            p99: percentile(99.0),
            max: *values.last().unwrap(),
            sum: values.iter().sum(),
            count: n as u64,
        }
    }

    fn ratio_max_over_median(&self) -> f64 {
        if self.median == 0 {
            // Anything > 0 with median 0 is infinite skew — clamp to a finite
            // sentinel so the threshold check still fires.
            if self.max > 0 { 1000.0 } else { 0.0 }
        } else {
            self.max as f64 / self.median as f64
        }
    }

    fn to_json(&self) -> serde_json::Value {
        json!({
            "median": self.median,
            "p95": self.p95,
            "p99": self.p99,
            "max": self.max,
            "sum": self.sum,
            "count": self.count,
        })
    }
}

fn partition_key(partition: &HashMap<String, String>) -> String {
    let mut parts: Vec<_> = partition.iter().collect();
    parts.sort_by_key(|(k, _)| (*k).clone());
    parts
        .iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join("/")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::metadata::*;
    use crate::test_helpers::make_test_metadata;
    use std::collections::HashMap;

    #[test]
    fn balanced_partitions_pass() {
        let mut meta = make_test_metadata();
        meta.partition_spec = PartitionSpec {
            spec_id: 0,
            fields: vec![PartitionField {
                source_id: 1,
                field_id: 1000,
                name: "date".to_string(),
                transform: "day".to_string(),
            }],
        };
        meta.data_files = (0..30)
            .map(|i| {
                let mut partition = HashMap::new();
                partition.insert("date".to_string(), format!("2026-01-{:02}", (i % 3) + 1));
                DataFile {
                    file_path: format!("s3://bucket/data/part-{i}.parquet"),
                    file_size_bytes: 100 * 1024 * 1024,
                    record_count: 1_000_000,
                    partition,
                    file_format: FileFormat::Parquet,
                    ..Default::default()
                }
            })
            .collect();

        let finding = PartitionSkewCheck.check(&meta, &Thresholds::default());
        assert_eq!(finding.severity, Severity::Pass);
    }

    #[test]
    fn detects_byte_skew_even_with_balanced_file_count() {
        // Each partition has 5 files, but partition "hot" holds 100x the bytes.
        let mut meta = make_test_metadata();
        meta.partition_spec = PartitionSpec {
            spec_id: 0,
            fields: vec![PartitionField {
                source_id: 1,
                field_id: 1000,
                name: "date".to_string(),
                transform: "day".to_string(),
            }],
        };

        let mut files = Vec::new();
        for p in 0..3 {
            for i in 0..5 {
                let mut partition = HashMap::new();
                partition.insert("date".to_string(), format!("2026-01-{:02}", p + 1));
                let bytes = if p == 0 {
                    5 * 1024 * 1024 * 1024
                } else {
                    50 * 1024 * 1024
                };
                files.push(DataFile {
                    file_path: format!("s3://bucket/data/part-{p}-{i}.parquet"),
                    file_size_bytes: bytes,
                    record_count: 1_000_000,
                    partition,
                    file_format: FileFormat::Parquet,
                    ..Default::default()
                });
            }
        }
        meta.data_files = files;

        let finding = PartitionSkewCheck.check(&meta, &Thresholds::default());
        assert!(finding.severity >= Severity::Warning);
        assert!(finding.message.contains("bytes") || finding.message.contains("by bytes"));
    }

    #[test]
    fn detects_file_count_skew() {
        let mut meta = make_test_metadata();
        meta.partition_spec = PartitionSpec {
            spec_id: 0,
            fields: vec![PartitionField {
                source_id: 1,
                field_id: 1000,
                name: "date".to_string(),
                transform: "day".to_string(),
            }],
        };

        let mut files = Vec::new();
        for i in 0..30 {
            let mut partition = HashMap::new();
            partition.insert("date".to_string(), format!("2026-01-{:02}", (i % 3) + 1));
            files.push(DataFile {
                file_path: format!("s3://bucket/data/part-{i}.parquet"),
                file_size_bytes: 100 * 1024 * 1024,
                record_count: 1_000_000,
                partition,
                file_format: FileFormat::Parquet,
                ..Default::default()
            });
        }
        for i in 30..230 {
            let mut partition = HashMap::new();
            partition.insert("date".to_string(), "2026-01-04".to_string());
            files.push(DataFile {
                file_path: format!("s3://bucket/data/part-{i}.parquet"),
                file_size_bytes: 100 * 1024 * 1024,
                record_count: 1_000_000,
                partition,
                file_format: FileFormat::Parquet,
                ..Default::default()
            });
        }
        meta.data_files = files;

        let finding = PartitionSkewCheck.check(&meta, &Thresholds::default());
        assert!(finding.severity >= Severity::Warning);
        assert!(finding.message.contains("2026-01-04"));
    }
}
