use clap::{Parser, Subcommand, ValueEnum};
use colored::Colorize;
use comfy_table::{Cell, Color, Table};
use frost_core::config::{FrostConfig, OutputFormat};
use frost_core::cost;
use frost_core::engine;
use frost_core::fix;
use frost_core::metadata::TableMetadata;
use frost_core::report::{HealthReport, Severity};

#[derive(Parser)]
#[command(
    name = "frost",
    about = "Iceberg table health for humans and agents",
    version,
    propagate_version = true
)]
struct Cli {
    /// Path to frost.toml config file.
    #[arg(long, global = true)]
    config: Option<String>,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Run health checks on an Iceberg table.
    Check {
        /// Table identifier (e.g., "db.events" or path to metadata).
        table: String,

        /// Output format.
        #[arg(long, short, value_enum, default_value = "pretty")]
        format: Format,

        /// Run only specific checks (comma-separated IDs).
        #[arg(long)]
        checks: Option<String>,
    },
    /// Show estimated cost waste for a table.
    Cost {
        /// Table identifier.
        table: String,

        /// Output format.
        #[arg(long, short, value_enum, default_value = "pretty")]
        format: Format,
    },
    /// Generate a fix command for a specific finding.
    Fix {
        /// Table identifier.
        table: String,
        /// Finding ID (e.g., "small_files", "snapshot_bloat").
        finding: String,
    },
}

#[derive(Clone, ValueEnum)]
enum Format {
    Pretty,
    Json,
    GithubActions,
}

impl From<Format> for OutputFormat {
    fn from(f: Format) -> Self {
        match f {
            Format::Pretty => OutputFormat::Pretty,
            Format::Json => OutputFormat::Json,
            Format::GithubActions => OutputFormat::GithubActions,
        }
    }
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "frost=info".parse().unwrap()),
        )
        .init();

    let cli = Cli::parse();

    let config = match &cli.config {
        Some(path) => match FrostConfig::from_file(std::path::Path::new(path)) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("{} {}", "error:".red().bold(), e);
                std::process::exit(1);
            }
        },
        None => FrostConfig::default(),
    };

    match cli.command {
        Commands::Check {
            table,
            format,
            checks,
        } => {
            let metadata = load_metadata_or_demo(&table);
            let report = if let Some(check_ids) = checks {
                let ids: Vec<&str> = check_ids.split(',').collect();
                engine::check_table_filtered(&metadata, &config.thresholds, &ids)
            } else {
                engine::check_table(&metadata, &config)
            };
            render_report(&report, format.into());
        }
        Commands::Cost { table, format } => {
            let metadata = load_metadata_or_demo(&table);
            let report = cost::estimate_cost(&metadata, &config.cost);
            match format {
                Format::Json => {
                    println!("{}", serde_json::to_string_pretty(&report).unwrap());
                }
                _ => render_cost_report(&report),
            }
        }
        Commands::Fix { table, finding } => {
            let metadata = load_metadata_or_demo(&table);
            match fix::generate_fix(&metadata, &finding) {
                Some(cmd) => {
                    println!("{}", "Fix Command".bold().underline());
                    println!();
                    println!("  {}", cmd.command.green());
                    println!();
                    println!("{}", cmd.description);
                    if !cmd.warnings.is_empty() {
                        println!();
                        println!("{}", "Warnings:".yellow().bold());
                        for w in &cmd.warnings {
                            println!("  - {}", w);
                        }
                    }
                }
                None => {
                    eprintln!(
                        "{} No fix available for finding '{}'",
                        "error:".red().bold(),
                        finding
                    );
                    std::process::exit(1);
                }
            }
        }
    }
}

/// Load table metadata from a catalog, or fall back to a demo table.
///
/// In Phase 1, catalog loading is not yet implemented, so we generate a
/// realistic demo table to show what the output looks like.
fn load_metadata_or_demo(table_identifier: &str) -> TableMetadata {
    use chrono::Utc;
    use frost_core::metadata::*;
    use std::collections::HashMap;

    tracing::info!("Loading metadata for '{}'", table_identifier);

    // Generate a demo table with realistic pathologies for demonstration.
    let now = Utc::now();

    let mut data_files = Vec::new();
    // 50 normal files
    for i in 0..50 {
        let mut partition = HashMap::new();
        partition.insert("date".to_string(), format!("2026-03-{:02}", (i % 28) + 1));
        data_files.push(DataFile {
            file_path: format!(
                "s3://demo-bucket/warehouse/{}/data/date=2026-03-{:02}/part-{:05}.parquet",
                table_identifier,
                (i % 28) + 1,
                i
            ),
            file_size_bytes: 128 * 1024 * 1024, // 128MB
            record_count: 1_500_000,
            partition,
            file_format: FileFormat::Parquet,
        });
    }
    // 25 small files (micro-batch residue)
    for i in 50..75 {
        let mut partition = HashMap::new();
        partition.insert("date".to_string(), "2026-04-01".to_string());
        data_files.push(DataFile {
            file_path: format!(
                "s3://demo-bucket/warehouse/{}/data/date=2026-04-01/part-{:05}.parquet",
                table_identifier, i
            ),
            file_size_bytes: 512 * 1024, // 512KB — small file
            record_count: 500,
            partition,
            file_format: FileFormat::Parquet,
        });
    }

    // Storage paths include all data files plus 3 orphans
    let mut all_storage_paths: Vec<String> =
        data_files.iter().map(|f| f.file_path.clone()).collect();
    all_storage_paths.push(format!(
        "s3://demo-bucket/warehouse/{}/data/orphan-001.parquet",
        table_identifier
    ));
    all_storage_paths.push(format!(
        "s3://demo-bucket/warehouse/{}/data/orphan-002.parquet",
        table_identifier
    ));
    all_storage_paths.push(format!(
        "s3://demo-bucket/warehouse/{}/data/orphan-003.parquet",
        table_identifier
    ));

    let snapshots: Vec<Snapshot> = (0..120)
        .map(|i| Snapshot {
            snapshot_id: i + 1,
            timestamp_ms: (now - chrono::Duration::hours(i * 2)).timestamp_millis(),
            summary: Default::default(),
            manifest_list: format!(
                "s3://demo-bucket/warehouse/{}/metadata/snap-{}-manifest-list.avro",
                table_identifier,
                i + 1
            ),
        })
        .collect();

    TableMetadata {
        table_name: table_identifier.to_string(),
        location: format!("s3://demo-bucket/warehouse/{}", table_identifier),
        current_schema: Schema {
            schema_id: 1,
            fields: vec![
                Field { id: 1, name: "id".to_string(), field_type: "long".to_string(), required: true },
                Field { id: 2, name: "user_id".to_string(), field_type: "long".to_string(), required: true },
                Field { id: 3, name: "event_type".to_string(), field_type: "string".to_string(), required: true },
                Field { id: 4, name: "payload".to_string(), field_type: "string".to_string(), required: false },
                Field { id: 5, name: "created_at".to_string(), field_type: "timestamp".to_string(), required: true },
            ],
        },
        schemas: vec![
            Schema {
                schema_id: 0,
                fields: vec![
                    Field { id: 1, name: "id".to_string(), field_type: "long".to_string(), required: true },
                    Field { id: 2, name: "user_id".to_string(), field_type: "long".to_string(), required: true },
                    Field { id: 3, name: "event_type".to_string(), field_type: "string".to_string(), required: true },
                    Field { id: 4, name: "created_at".to_string(), field_type: "timestamp".to_string(), required: true },
                ],
            },
            Schema {
                schema_id: 1,
                fields: vec![
                    Field { id: 1, name: "id".to_string(), field_type: "long".to_string(), required: true },
                    Field { id: 2, name: "user_id".to_string(), field_type: "long".to_string(), required: true },
                    Field { id: 3, name: "event_type".to_string(), field_type: "string".to_string(), required: true },
                    Field { id: 4, name: "payload".to_string(), field_type: "string".to_string(), required: false },
                    Field { id: 5, name: "created_at".to_string(), field_type: "timestamp".to_string(), required: true },
                ],
            },
        ],
        snapshots,
        current_snapshot_id: Some(120),
        partition_spec: PartitionSpec {
            spec_id: 0,
            fields: vec![PartitionField {
                source_id: 5,
                field_id: 1000,
                name: "date".to_string(),
                transform: "day".to_string(),
            }],
        },
        sort_order: None,
        data_files,
        delete_files: vec![],
        all_storage_paths,
        metadata_size_bytes: 45 * 1024 * 1024, // 45MB metadata
        collected_at: now,
    }
}

fn render_report(report: &HealthReport, format: OutputFormat) {
    match format {
        OutputFormat::Json => {
            println!("{}", serde_json::to_string_pretty(report).unwrap());
        }
        OutputFormat::GithubActions => {
            render_github_actions(report);
        }
        OutputFormat::Pretty => {
            render_pretty(report);
        }
    }
}

fn render_pretty(report: &HealthReport) {
    let size_str = format_bytes(report.summary.total_size_bytes);
    let records_str = format_number(report.summary.total_record_count);

    println!();
    println!(
        "{}",
        "╔══════════════════════════════════════════════════════════════╗"
            .cyan()
            .bold()
    );
    println!(
        "{}  {} · Iceberg Table Health Report{}",
        "║".cyan().bold(),
        "frost".bold().cyan(),
        " ".repeat(24).to_string() + &"║".cyan().bold().to_string()
    );
    println!(
        "{}  Table: {:<51}{}",
        "║".cyan().bold(),
        report.table_name,
        "║".cyan().bold()
    );
    println!(
        "{}  Snapshots: {} · Data Files: {} · Size: {:<19}{}",
        "║".cyan().bold(),
        report.summary.snapshot_count,
        report.summary.data_file_count,
        size_str,
        "║".cyan().bold()
    );
    println!(
        "{}  Records: {:<50}{}",
        "║".cyan().bold(),
        records_str,
        "║".cyan().bold()
    );
    println!(
        "{}",
        "╠══════════════════════════════════════════════════════════════╣"
            .cyan()
            .bold()
    );

    for finding in &report.findings {
        let icon = match finding.severity {
            Severity::Pass => "✅".to_string(),
            Severity::Warning => "⚠ ".to_string(),
            Severity::Critical => "🔴".to_string(),
        };

        let name_colored = match finding.severity {
            Severity::Pass => finding.check_name.green().to_string(),
            Severity::Warning => finding.check_name.yellow().to_string(),
            Severity::Critical => finding.check_name.red().bold().to_string(),
        };

        println!(
            "{}  {} {:<20} {}",
            "║".cyan().bold(),
            icon,
            name_colored,
            finding.message,
        );

        if finding.severity != Severity::Pass {
            if !finding.impact.is_empty() {
                println!(
                    "{}     {} {}",
                    "║".cyan().bold(),
                    "Impact:".dimmed(),
                    finding.impact.dimmed()
                );
            }
            if let Some(ref fix) = finding.fix_suggestion {
                println!(
                    "{}     {} {}",
                    "║".cyan().bold(),
                    "Fix:".dimmed(),
                    fix.dimmed()
                );
            }
            if let Some(ref savings) = finding.estimated_savings {
                println!(
                    "{}     {} {}",
                    "║".cyan().bold(),
                    "Savings:".dimmed(),
                    savings.dimmed()
                );
            }
        }
        println!("{}",  "║".cyan().bold());
    }

    let overall_str = match report.overall.severity {
        Severity::Pass => "All checks passed".green().bold().to_string(),
        Severity::Warning => format!(
            "{} warning(s), {} critical",
            report.overall.warning_count, report.overall.critical_count
        )
        .yellow()
        .bold()
        .to_string(),
        Severity::Critical => format!(
            "{} warning(s), {} critical",
            report.overall.warning_count, report.overall.critical_count
        )
        .red()
        .bold()
        .to_string(),
    };

    println!(
        "{}  Overall: {}",
        "║".cyan().bold(),
        overall_str,
    );
    println!(
        "{}",
        "╚══════════════════════════════════════════════════════════════╝"
            .cyan()
            .bold()
    );
    println!();
}

fn render_github_actions(report: &HealthReport) {
    for finding in &report.findings {
        match finding.severity {
            Severity::Pass => {}
            Severity::Warning => {
                println!(
                    "::warning title=frost: {}::{}",
                    finding.check_name, finding.message
                );
            }
            Severity::Critical => {
                println!(
                    "::error title=frost: {}::{}",
                    finding.check_name, finding.message
                );
            }
        }
    }

    if report.overall.critical_count > 0 {
        std::process::exit(1);
    }
}

fn render_cost_report(report: &cost::CostReport) {
    println!();
    println!(
        "{} for {}",
        "Cost Waste Estimate".bold().underline(),
        report.table_name.bold()
    );
    println!();

    if report.items.is_empty() {
        println!("  {} No cost waste detected", "✅".to_string());
    } else {
        let mut table = Table::new();
        table.set_header(vec![
            Cell::new("Category").fg(Color::Cyan),
            Cell::new("Description").fg(Color::Cyan),
            Cell::new("Monthly Cost").fg(Color::Cyan),
        ]);

        for item in &report.items {
            table.add_row(vec![
                Cell::new(&item.category),
                Cell::new(&item.description),
                Cell::new(format!("${:.4}", item.monthly_cost)),
            ]);
        }

        println!("{table}");
    }

    println!();
    println!(
        "  Total estimated monthly waste: {}",
        format!("${:.4}", report.total_monthly_waste)
            .yellow()
            .bold()
    );
    println!();
}

fn format_bytes(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    const GB: u64 = MB * 1024;
    const TB: u64 = GB * 1024;

    if bytes >= TB {
        format!("{:.1} TB", bytes as f64 / TB as f64)
    } else if bytes >= GB {
        format!("{:.1} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.1} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.1} KB", bytes as f64 / KB as f64)
    } else {
        format!("{} B", bytes)
    }
}

fn format_number(n: u64) -> String {
    if n >= 1_000_000_000 {
        format!("{:.1}B", n as f64 / 1_000_000_000.0)
    } else if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{:.1}K", n as f64 / 1_000.0)
    } else {
        n.to_string()
    }
}
