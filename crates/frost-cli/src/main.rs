use clap::{Parser, Subcommand, ValueEnum};
use colored::Colorize;
use comfy_table::{Cell, Color, Table};
use frost_core::catalog;
use frost_core::config::{CatalogConfig, FrostConfig, OutputFormat};
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

    /// Warehouse path (overrides config). Enables filesystem catalog.
    #[arg(long, short, global = true)]
    warehouse: Option<String>,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Run health checks on an Iceberg table.
    Check {
        /// Table identifier (e.g., "db.events") or path to table directory.
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
    /// List all tables in a catalog.
    List {
        /// Optional namespace filter.
        namespace: Option<String>,

        /// Output format.
        #[arg(long, short, value_enum, default_value = "pretty")]
        format: Format,
    },
    /// Start watch mode — periodically check all tables and alert on changes.
    Watch {
        /// Check interval (e.g., "30m", "1h", "6h"). Overrides config.
        #[arg(long)]
        interval: Option<String>,

        /// Webhook URL for alerts.
        #[arg(long)]
        webhook: Option<String>,

        /// Optional namespace filter.
        #[arg(long)]
        namespace: Option<String>,

        /// SQLite database path for state.
        #[arg(long, default_value = "./frost-watch.db")]
        db: String,
    },
    /// Show current watch mode status from the state database.
    WatchStatus {
        /// Optional table filter.
        #[arg(long)]
        table: Option<String>,

        /// SQLite database path.
        #[arg(long, default_value = "./frost-watch.db")]
        db: String,

        /// Output format.
        #[arg(long, short, value_enum, default_value = "pretty")]
        format: Format,
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

    let mut config = match &cli.config {
        Some(path) => match FrostConfig::from_file(std::path::Path::new(path)) {
            Ok(c) => c,
            Err(e) => {
                eprintln!("{} {}", "error:".red().bold(), e);
                std::process::exit(1);
            }
        },
        None => {
            // Auto-detect frost.toml in current directory.
            let local = std::path::Path::new("frost.toml");
            if local.exists() {
                FrostConfig::from_file(local).unwrap_or_default()
            } else {
                FrostConfig::default()
            }
        }
    };

    // --warehouse flag overrides catalog config.
    if let Some(ref warehouse) = cli.warehouse {
        config.catalog = CatalogConfig::Filesystem {
            warehouse: warehouse.clone(),
        };
    }

    match cli.command {
        Commands::Check {
            table,
            format,
            checks,
        } => {
            let metadata = load_metadata(&table, &config).await;
            let report = if let Some(check_ids) = checks {
                let ids: Vec<&str> = check_ids.split(',').collect();
                engine::check_table_filtered(&metadata, &config.thresholds, &ids)
            } else {
                engine::check_table(&metadata, &config)
            };
            render_report(&report, format.into());

            // Exit with non-zero status if there are critical findings (for CI).
            if report.overall.critical_count > 0 {
                std::process::exit(1);
            }
        }
        Commands::Cost { table, format } => {
            let metadata = load_metadata(&table, &config).await;
            let report = cost::estimate_cost(&metadata, &config.cost);
            match format {
                Format::Json => {
                    println!("{}", serde_json::to_string_pretty(&report).unwrap());
                }
                _ => render_cost_report(&report),
            }
        }
        Commands::Fix { table, finding } => {
            let metadata = load_metadata(&table, &config).await;
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
        Commands::Watch {
            interval,
            webhook,
            namespace,
            db,
        } => {
            // Override watch config from CLI flags.
            if let Some(interval) = interval {
                config.watch.interval = interval;
            }
            if let Some(webhook) = webhook {
                config.watch.webhook_url = Some(webhook);
            }
            if namespace.is_some() {
                config.watch.namespace = namespace;
            }
            config.watch.sqlite_path = db;

            // Validate interval before starting.
            if let Err(e) = frost_core::watch::parse_interval(&config.watch.interval) {
                eprintln!("{} {}", "error:".red().bold(), e);
                std::process::exit(1);
            }

            println!(
                "{} Starting watch mode (interval: {}, db: {})",
                "frost".cyan().bold(),
                config.watch.interval,
                config.watch.sqlite_path
            );
            if let Some(ref url) = config.watch.webhook_url {
                println!("  Webhook: {}", url);
            }
            if let Some(ref ns) = config.watch.namespace {
                println!("  Namespace filter: {}", ns);
            }
            println!("  Press Ctrl+C to stop.");
            println!();

            if let Err(e) = frost_core::watch::run_watch_loop(&config).await {
                eprintln!("{} Watch mode failed: {}", "error:".red().bold(), e);
                std::process::exit(1);
            }
        }
        Commands::WatchStatus { table, db, format } => {
            let watch_db = match frost_core::watch::WatchDb::open(&db) {
                Ok(db) => db,
                Err(e) => {
                    eprintln!("{} Failed to open watch database: {}", "error:".red().bold(), e);
                    std::process::exit(1);
                }
            };

            match format {
                Format::Json => {
                    let latest = if let Some(ref t) = table {
                        watch_db
                            .get_latest_report(t)
                            .unwrap_or(None)
                            .into_iter()
                            .collect::<Vec<_>>()
                    } else {
                        watch_db.get_all_latest().unwrap_or_default()
                    };
                    let alerts = watch_db.get_alerts(table.as_deref(), 10).unwrap_or_default();

                    let output = serde_json::json!({
                        "tables": latest,
                        "recent_alerts": alerts,
                    });
                    println!("{}", serde_json::to_string_pretty(&output).unwrap());
                }
                _ => {
                    let latest = if let Some(ref t) = table {
                        watch_db
                            .get_latest_report(t)
                            .unwrap_or(None)
                            .into_iter()
                            .collect::<Vec<_>>()
                    } else {
                        watch_db.get_all_latest().unwrap_or_default()
                    };

                    if latest.is_empty() {
                        println!("No watch data found. Run {} to start monitoring.", "frost watch".bold());
                    } else {
                        println!("{}", "Watch Status".bold().underline());
                        println!();
                        for report in &latest {
                            let severity_colored = match report.severity.as_str() {
                                "PASS" => report.severity.green().to_string(),
                                "WARNING" => report.severity.yellow().to_string(),
                                "CRITICAL" => report.severity.red().bold().to_string(),
                                _ => report.severity.clone(),
                            };
                            println!(
                                "  {} — {} ({} findings, last checked: {})",
                                report.table_name.bold(),
                                severity_colored,
                                report.finding_count,
                                report.checked_at.format("%Y-%m-%d %H:%M UTC"),
                            );
                        }

                        let alerts = watch_db.get_alerts(table.as_deref(), 5).unwrap_or_default();
                        if !alerts.is_empty() {
                            println!();
                            println!("{}", "Recent Alerts".bold().underline());
                            for alert in &alerts {
                                println!(
                                    "  [{}] {} — {}",
                                    alert.alerted_at.format("%Y-%m-%d %H:%M"),
                                    alert.table_name,
                                    alert.message
                                );
                            }
                        }
                    }
                }
            }
        }
        Commands::List { namespace, format } => {
            let provider = match catalog::from_config(&config.catalog) {
                Ok(p) => p,
                Err(e) => {
                    eprintln!("{} {}", "error:".red().bold(), e);
                    std::process::exit(1);
                }
            };

            let tables = match provider.list_tables(namespace.as_deref()).await {
                Ok(t) => t,
                Err(e) => {
                    eprintln!("{} {}", "error:".red().bold(), e);
                    std::process::exit(1);
                }
            };

            match format {
                Format::Json => {
                    println!("{}", serde_json::to_string_pretty(&tables).unwrap());
                }
                _ => {
                    if tables.is_empty() {
                        println!("No tables found.");
                    } else {
                        println!(
                            "{} ({} tables)",
                            "Tables".bold().underline(),
                            tables.len()
                        );
                        for table in &tables {
                            println!("  {}", table);
                        }
                    }
                }
            }
        }
    }
}

/// Load table metadata — tries the configured catalog first, falls back to demo data.
async fn load_metadata(table_identifier: &str, config: &FrostConfig) -> TableMetadata {
    // Try loading from configured catalog.
    match catalog::from_config(&config.catalog) {
        Ok(provider) => match provider.load_table(table_identifier).await {
            Ok(meta) => {
                tracing::info!(
                    "Loaded '{}' from catalog ({} data files, {} snapshots)",
                    table_identifier,
                    meta.data_files.len(),
                    meta.snapshots.len(),
                );
                return meta;
            }
            Err(e) => {
                tracing::debug!("Catalog load failed: {}, falling back to demo data", e);
            }
        },
        Err(e) => {
            tracing::debug!("Catalog not configured: {}, using demo data", e);
        }
    }

    // Fall back to demo data.
    tracing::info!(
        "Using demo data for '{}' (configure --warehouse or frost.toml for real tables)",
        table_identifier
    );
    generate_demo_metadata(table_identifier)
}

/// Generate a demo table with realistic pathologies for demonstration.
fn generate_demo_metadata(table_identifier: &str) -> TableMetadata {
    use chrono::Utc;
    use frost_core::metadata::*;
    use std::collections::HashMap;

    let now = Utc::now();

    let mut data_files = Vec::new();
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
            file_size_bytes: 128 * 1024 * 1024,
            record_count: 1_500_000,
            partition,
            file_format: FileFormat::Parquet,
        });
    }
    for i in 50..75 {
        let mut partition = HashMap::new();
        partition.insert("date".to_string(), "2026-04-01".to_string());
        data_files.push(DataFile {
            file_path: format!(
                "s3://demo-bucket/warehouse/{}/data/date=2026-04-01/part-{:05}.parquet",
                table_identifier, i
            ),
            file_size_bytes: 512 * 1024,
            record_count: 500,
            partition,
            file_format: FileFormat::Parquet,
        });
    }

    let mut all_storage_paths: Vec<String> =
        data_files.iter().map(|f| f.file_path.clone()).collect();
    for i in 0..3 {
        all_storage_paths.push(format!(
            "s3://demo-bucket/warehouse/{}/data/orphan-{:03}.parquet",
            table_identifier, i
        ));
    }

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
        metadata_size_bytes: 45 * 1024 * 1024,
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
        println!("{}", "║".cyan().bold());
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

    println!("{}  Overall: {}", "║".cyan().bold(), overall_str);
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
        println!("  {} No cost waste detected", "✅");
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
