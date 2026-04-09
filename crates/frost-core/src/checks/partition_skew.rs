use crate::checks::HealthCheck;
use crate::config::Thresholds;
use crate::metadata::TableMetadata;
use crate::report::{Finding, Severity};
use serde_json::json;
use std::collections::HashMap;

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

        // Count files per partition value (using the string representation of partition values).
        let mut partition_counts: HashMap<String, u64> = HashMap::new();
        for file in &metadata.data_files {
            let key = partition_key(&file.partition);
            *partition_counts.entry(key).or_default() += 1;
        }

        if partition_counts.is_empty() {
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

        let mut counts: Vec<u64> = partition_counts.values().copied().collect();
        counts.sort_unstable();

        let median = counts[counts.len() / 2];
        let max = *counts.last().unwrap();
        let max_partition = partition_counts
            .iter()
            .find(|(_, v)| **v == max)
            .map(|(k, _)| k.clone())
            .unwrap_or_default();

        let ratio = if median > 0 {
            max as f64 / median as f64
        } else {
            max as f64
        };

        if ratio <= thresholds.partition_skew_ratio {
            return Finding {
                check_id: self.id().to_string(),
                check_name: self.name().to_string(),
                severity: Severity::Pass,
                message: format!(
                    "Partition file counts look balanced (max/median ratio: {:.1}x)",
                    ratio,
                ),
                impact: String::new(),
                fix_suggestion: None,
                fix_command: None,
                estimated_savings: None,
                details: json!({
                    "partition_count": counts.len(),
                    "median_files": median,
                    "max_files": max,
                    "max_partition": max_partition,
                    "skew_ratio": format!("{:.1}", ratio),
                }),
            };
        }

        let severity = if ratio > thresholds.partition_skew_ratio * 5.0 {
            Severity::Critical
        } else {
            Severity::Warning
        };

        Finding {
            check_id: self.id().to_string(),
            check_name: self.name().to_string(),
            severity,
            message: format!(
                "Partition '{}' has {} files vs median of {} files per partition ({:.1}x skew)",
                max_partition, max, median, ratio,
            ),
            impact: "Hot partitions cause Spark task stragglers — one executor does \
                     disproportionate work, turning a fast job into a slow one."
                .to_string(),
            fix_suggestion: Some(
                "Consider repartitioning or sort-order optimization to distribute data more evenly"
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
                "Reduced task straggler time by balancing partition '{}'",
                max_partition,
            )),
            details: json!({
                "partition_count": counts.len(),
                "median_files": median,
                "max_files": max,
                "max_partition": max_partition,
                "skew_ratio": format!("{:.1}", ratio),
            }),
        }
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
                }
            })
            .collect();

        let finding = PartitionSkewCheck.check(&meta, &Thresholds::default());
        assert_eq!(finding.severity, Severity::Pass);
    }

    #[test]
    fn detects_skew() {
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
        // 3 partitions with 10 files each
        for i in 0..30 {
            let mut partition = HashMap::new();
            partition.insert("date".to_string(), format!("2026-01-{:02}", (i % 3) + 1));
            files.push(DataFile {
                file_path: format!("s3://bucket/data/part-{i}.parquet"),
                file_size_bytes: 100 * 1024 * 1024,
                record_count: 1_000_000,
                partition,
                file_format: FileFormat::Parquet,
            });
        }
        // 1 hot partition with 200 extra files
        for i in 30..230 {
            let mut partition = HashMap::new();
            partition.insert("date".to_string(), "2026-01-04".to_string());
            files.push(DataFile {
                file_path: format!("s3://bucket/data/part-{i}.parquet"),
                file_size_bytes: 100 * 1024 * 1024,
                record_count: 1_000_000,
                partition,
                file_format: FileFormat::Parquet,
            });
        }
        meta.data_files = files;

        let finding = PartitionSkewCheck.check(&meta, &Thresholds::default());
        assert!(finding.severity >= Severity::Warning);
        assert!(finding.message.contains("2026-01-04"));
    }
}
