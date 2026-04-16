use clap::Parser;
use frost_core::config::{CatalogConfig, FrostConfig};
use frost_mcp::server::FrostServer;
use rmcp::ServiceExt;

#[derive(Parser)]
#[command(
    name = "frost-mcp",
    about = "MCP server for frost — Iceberg table health tools for AI agents",
    version
)]
struct Cli {
    /// Path to frost.toml config file.
    #[arg(long)]
    config: Option<String>,

    /// Warehouse path (overrides config). Enables filesystem catalog.
    #[arg(long, short)]
    warehouse: Option<String>,

    /// Transport: "stdio" (default) or "sse".
    #[arg(long, default_value = "stdio")]
    transport: String,

    /// Port for SSE transport (default: 3000).
    #[arg(long, default_value = "3000")]
    port: u16,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Log to stderr so stdout is clean for MCP protocol.
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "frost_mcp=info".parse().unwrap()),
        )
        .init();

    let cli = Cli::parse();

    let mut config = match &cli.config {
        Some(path) => FrostConfig::from_file(std::path::Path::new(path))?,
        None => {
            let local = std::path::Path::new("frost.toml");
            if local.exists() {
                FrostConfig::from_file(local).unwrap_or_default()
            } else {
                FrostConfig::default()
            }
        }
    };

    if let Some(ref warehouse) = cli.warehouse {
        config.catalog = CatalogConfig::Filesystem {
            warehouse: warehouse.clone(),
        };
    }

    let server = FrostServer::new(config);

    match cli.transport.as_str() {
        "stdio" => {
            tracing::info!("Starting frost MCP server on stdio transport");
            let service = server.serve(rmcp::transport::stdio()).await?;
            service.waiting().await?;
        }
        other => {
            eprintln!("Unknown transport: '{}'. Supported: stdio", other);
            std::process::exit(1);
        }
    }

    Ok(())
}
