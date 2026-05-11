use crate::checks::HealthCheck;
use crate::config::Thresholds;
use crate::metadata::TableMetadata;
use crate::report::{Finding, Severity};
use serde_json::json;
use std::collections::HashSet;

/// Detects partition-spec churn.
///
/// Each retired spec leaves data files behind that the planner has to
/// re-resolve at query time. Iceberg supports unbounded partition
/// evolution, but each new spec has a real planner cost. We flag tables
/// with too many distinct specs OR with files still resident under
/// retired specs.
pub struct PartitionSpecEvolutionCheck;

impl HealthCheck for PartitionSpecEvolutionCheck {
    fn id(&self) -> &'static str {
        "partition_spec_evolution"
    }

    fn name(&self) -> &'static str {
        "Partition Spec Evolution"
    }

    fn check(&self, metadata: &TableMetadata, thresholds: &Thresholds) -> Finding {
        let total_specs = metadata.partition_specs.len();
        let default_spec_id = metadata.partition_spec.spec_id;

        // Count files written under non-default specs (specs the table has
        // since evolved past). spec_id is None on older writers — those
        // count as "unknown" and are excluded.
        let mut files_under_old_specs: u64 = 0;
        let mut old_spec_ids: HashSet<i32> = HashSet::new();
        for f in &metadata.data_files {
            if let Some(sid) = f.spec_id
                && sid != default_spec_id
            {
                files_under_old_specs += 1;
                old_spec_ids.insert(sid);
            }
        }

        let too_many_specs = total_specs > thresholds.max_partition_specs;
        let has_orphaned_spec_files = files_under_old_specs > 0;

        if !too_many_specs && !has_orphaned_spec_files {
            return Finding {
                check_id: self.id().to_string(),
                check_name: self.name().to_string(),
                severity: Severity::Pass,
                message: format!(
                    "{} partition spec(s) declared, all data under default spec {}",
                    total_specs, default_spec_id,
                ),
                impact: String::new(),
                fix_suggestion: None,
                fix_command: None,
                estimated_savings: None,
                details: json!({
                    "total_specs": total_specs,
                    "default_spec_id": default_spec_id,
                    "files_under_old_specs": files_under_old_specs,
                }),
            };
        }

        let severity = if total_specs > thresholds.max_partition_specs * 2
            || files_under_old_specs > metadata.data_files.len() as u64 / 4
        {
            Severity::Critical
        } else {
            Severity::Warning
        };

        let mut parts = Vec::new();
        if too_many_specs {
            parts.push(format!(
                "{} partition specs (threshold: {})",
                total_specs, thresholds.max_partition_specs,
            ));
        }
        if has_orphaned_spec_files {
            parts.push(format!(
                "{} files still under {} retired spec(s)",
                files_under_old_specs,
                old_spec_ids.len(),
            ));
        }

        Finding {
            check_id: self.id().to_string(),
            check_name: self.name().to_string(),
            severity,
            message: parts.join("; "),
            impact: "Partition-spec evolution churn forces the query planner to resolve \
                     each retired spec separately, slowing planning. Files written under \
                     old specs must be rewritten under the current spec to fully retire it."
                .to_string(),
            fix_suggestion: Some(
                "Rewrite files under retired partition specs so the table can fully \
                 transition to the current spec. Avoid further spec changes unless \
                 absolutely necessary — repartitioning has a real planner cost."
                    .to_string(),
            ),
            fix_command: Some(format!(
                "CALL catalog.system.rewrite_data_files(table => '{}', \
                 where => 'true', options => map('rewrite-all', 'true'))",
                metadata.table_name,
            )),
            estimated_savings: None,
            details: json!({
                "total_specs": total_specs,
                "default_spec_id": default_spec_id,
                "files_under_old_specs": files_under_old_specs,
                "retired_spec_ids": old_spec_ids.iter().collect::<Vec<_>>(),
                "threshold_max_specs": thresholds.max_partition_specs,
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::metadata::{DataFile, FileFormat, PartitionSpec};
    use crate::test_helpers::make_test_metadata;

    #[test]
    fn single_spec_passes() {
        let meta = make_test_metadata();
        let f = PartitionSpecEvolutionCheck.check(&meta, &Thresholds::default());
        assert_eq!(f.severity, Severity::Pass);
    }

    #[test]
    fn flags_files_under_retired_spec() {
        let mut meta = make_test_metadata();
        meta.partition_specs = vec![
            PartitionSpec {
                spec_id: 0,
                fields: vec![],
            },
            PartitionSpec {
                spec_id: 1,
                fields: vec![],
            },
        ];
        meta.partition_spec = PartitionSpec {
            spec_id: 1,
            fields: vec![],
        };
        meta.data_files = vec![DataFile {
            file_path: "x".into(),
            file_size_bytes: 1024,
            record_count: 100,
            file_format: FileFormat::Parquet,
            spec_id: Some(0),
            ..Default::default()
        }];
        let f = PartitionSpecEvolutionCheck.check(&meta, &Thresholds::default());
        assert!(f.severity >= Severity::Warning);
    }

    #[test]
    fn flags_too_many_specs() {
        let mut meta = make_test_metadata();
        meta.partition_specs = (0..5)
            .map(|i| PartitionSpec {
                spec_id: i,
                fields: vec![],
            })
            .collect();
        let f = PartitionSpecEvolutionCheck.check(&meta, &Thresholds::default());
        assert!(f.severity >= Severity::Warning);
    }
}
