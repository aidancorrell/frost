use crate::checks::HealthCheck;
use crate::config::Thresholds;
use crate::metadata::TableMetadata;
use crate::report::{Finding, Severity};
use serde_json::json;
use std::collections::HashMap;

pub struct SchemaHistoryCheck;

impl HealthCheck for SchemaHistoryCheck {
    fn id(&self) -> &'static str {
        "schema_history"
    }

    fn name(&self) -> &'static str {
        "Schema Evolution"
    }

    fn check(&self, metadata: &TableMetadata, _thresholds: &Thresholds) -> Finding {
        if metadata.schemas.len() <= 1 {
            return Finding {
                check_id: self.id().to_string(),
                check_name: self.name().to_string(),
                severity: Severity::Pass,
                message: "Single schema version — no evolution to analyze".to_string(),
                impact: String::new(),
                fix_suggestion: None,
                fix_command: None,
                estimated_savings: None,
                details: json!({ "schema_versions": metadata.schemas.len() }),
            };
        }

        let mut breaking_changes = Vec::new();
        let mut additive_changes = Vec::new();

        // Compare consecutive schema versions.
        for pair in metadata.schemas.windows(2) {
            let old = &pair[0];
            let new = &pair[1];

            let old_fields: HashMap<&str, &str> = old
                .fields
                .iter()
                .map(|f| (f.name.as_str(), f.field_type.as_str()))
                .collect();
            let new_fields: HashMap<&str, &str> = new
                .fields
                .iter()
                .map(|f| (f.name.as_str(), f.field_type.as_str()))
                .collect();

            // Dropped columns.
            for (name, _) in &old_fields {
                if !new_fields.contains_key(name) {
                    breaking_changes.push(format!(
                        "Column '{}' dropped (schema {} → {})",
                        name, old.schema_id, new.schema_id,
                    ));
                }
            }

            // Added columns.
            for (name, _) in &new_fields {
                if !old_fields.contains_key(name) {
                    additive_changes.push(format!(
                        "Column '{}' added (schema {} → {})",
                        name, old.schema_id, new.schema_id,
                    ));
                }
            }

            // Type changes.
            for (name, old_type) in &old_fields {
                if let Some(new_type) = new_fields.get(name) {
                    if old_type != new_type {
                        breaking_changes.push(format!(
                            "Column '{}' type changed: {} → {} (schema {} → {})",
                            name, old_type, new_type, old.schema_id, new.schema_id,
                        ));
                    }
                }
            }
        }

        if breaking_changes.is_empty() {
            return Finding {
                check_id: self.id().to_string(),
                check_name: self.name().to_string(),
                severity: Severity::Pass,
                message: format!(
                    "Clean schema evolution — {} versions, {} additive changes, no breaking changes",
                    metadata.schemas.len(),
                    additive_changes.len(),
                ),
                impact: String::new(),
                fix_suggestion: None,
                fix_command: None,
                estimated_savings: None,
                details: json!({
                    "schema_versions": metadata.schemas.len(),
                    "additive_changes": additive_changes,
                    "breaking_changes": [],
                }),
            };
        }

        let severity = if breaking_changes.len() > 3 {
            Severity::Critical
        } else {
            Severity::Warning
        };

        Finding {
            check_id: self.id().to_string(),
            check_name: self.name().to_string(),
            severity,
            message: format!(
                "{} breaking schema changes detected across {} versions",
                breaking_changes.len(),
                metadata.schemas.len(),
            ),
            impact: "Breaking schema changes (column drops, type changes) can break downstream \
                     consumers that haven't updated their readers."
                .to_string(),
            fix_suggestion: Some(
                "Review breaking changes and ensure all downstream consumers are compatible"
                    .to_string(),
            ),
            fix_command: None,
            estimated_savings: None,
            details: json!({
                "schema_versions": metadata.schemas.len(),
                "breaking_changes": breaking_changes,
                "additive_changes": additive_changes,
            }),
        }
    }
}
