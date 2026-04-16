use crate::checks::HealthCheck;
use crate::config::Thresholds;
use crate::metadata::TableMetadata;
use crate::report::{Finding, Severity};
use serde_json::json;

pub struct SortOrderCheck;

impl HealthCheck for SortOrderCheck {
    fn id(&self) -> &'static str {
        "sort_order"
    }

    fn name(&self) -> &'static str {
        "Sort Order Health"
    }

    fn check(&self, metadata: &TableMetadata, _thresholds: &Thresholds) -> Finding {
        let sort_order = match &metadata.sort_order {
            Some(so) if !so.fields.is_empty() => so,
            _ => {
                return Finding {
                    check_id: self.id().to_string(),
                    check_name: self.name().to_string(),
                    severity: Severity::Pass,
                    message: "No sort order declared — check not applicable".to_string(),
                    impact: String::new(),
                    fix_suggestion: None,
                    fix_command: None,
                    estimated_savings: None,
                    details: json!({ "has_sort_order": false }),
                };
            }
        };

        // Without reading actual Parquet file footers (which would violate the
        // metadata-only design), we can only report that a sort order is declared
        // and suggest validation. In a future version, we could sample manifest
        // entries' lower/upper bounds to infer sort compliance.
        let field_names: Vec<&str> = sort_order
            .fields
            .iter()
            .map(|f| f.transform.as_str())
            .collect();

        Finding {
            check_id: self.id().to_string(),
            check_name: self.name().to_string(),
            severity: Severity::Pass,
            message: format!(
                "Sort order declared with {} field(s): [{}]",
                sort_order.fields.len(),
                field_names.join(", "),
            ),
            impact: String::new(),
            fix_suggestion: Some(
                "Consider running rewrite_data_files with sort strategy if data was written \
                 without respecting the sort order"
                    .to_string(),
            ),
            fix_command: None,
            estimated_savings: None,
            details: json!({
                "has_sort_order": true,
                "sort_order_id": sort_order.order_id,
                "fields": sort_order.fields.iter().map(|f| json!({
                    "source_id": f.source_id,
                    "transform": &f.transform,
                    "direction": &f.direction,
                    "null_order": &f.null_order,
                })).collect::<Vec<_>>(),
            }),
        }
    }
}
