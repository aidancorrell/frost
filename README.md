# frost

**Agent-first Iceberg table health tool.** MCP server, CLI, and CI gate that inspects table metadata, diagnoses operational problems, estimates cost waste, and generates fix commands.

Designed for AI agents to call autonomously and humans to run interactively.

---

## Why frost?

Every team adopting Iceberg inherits operational problems that managed warehouses handle invisibly: small file accumulation, snapshot bloat, orphan files, partition skew, stale metadata, schema drift. Today the answer is "write a Spark job and eyeball it."

As AI agents increasingly write and maintain data pipelines, they need a way to **assess table health before and after they act.** An agent writing a Spark job should know the target table has a small file problem before it adds to it. An orchestration system should gate deployments on table health.

`frost` is that tool. Three interfaces, one engine:

1. **MCP server** (primary) — agents call `frost` tools mid-workflow
2. **CLI** — humans run `frost check` interactively or in scripts
3. **CI gate** — `frost` runs in GitHub Actions, failing on critical findings

## Key Design Decision: Metadata-Only

frost never reads data files. It only reads metadata (snapshot JSON, manifests, manifest lists). This means:

- **Fast** — checking a 100K-file table takes seconds
- **Cheap** — no compute cost, minimal S3 GET requests
- **Safe** — read-only, can't break anything
- **Agent-safe** — agents can call `check_table` on every table without cost risk

---

## Health Checks

| Check | What it detects | Severity |
|---|---|---|
| **small_files** | Files under 8 MB; reports bytes-trapped + partition hotspots | Warning/Critical |
| **snapshot_bloat** | Too many retained snapshots (>100) | Warning/Critical |
| **orphan_files** | Unreferenced files in storage | Warning |
| **partition_skew** | Skew in bytes/rows/files with p95/p99 distribution | Warning/Critical |
| **delete_pressure** | Equality vs position delete files weighted by row-shadow | Warning/Critical |
| **schema_history** | Dropped columns, type changes | Warning |
| **metadata_size** | Metadata exceeding 500 MB | Warning/Critical |
| **sort_order** | Sort order declared (presence) | Info |
| **freshness** | Hours since last snapshot commit | Warning |
| **format_v1** | Tables still on Iceberg format-version 1 | Warning |
| **properties_drift** | `write.target-file-size-bytes` declared vs observed median | Warning/Critical |
| **partition_spec_evolution** | Spec churn + files left under retired specs | Warning/Critical |
| **sort_compliance** | % of files honoring the declared sort order (uses `sort_order_id`) | Warning/Critical |
| **stats_coverage** | % of files carrying column statistics | Warning/Critical |
| **branch_health** | Per-branch/tag staleness; dangling refs | Warning/Critical |

All thresholds are configurable in `frost.toml`.

---

## Quick Start

### Install

```bash
# Pre-built binaries (Linux/macOS/Windows, x86_64 + aarch64) on every
# tagged release: https://github.com/aidancorrell/frost/releases

# Container image:
docker pull ghcr.io/aidancorrell/frost:latest

# From source:
git clone https://github.com/aidancorrell/frost && cd frost
cargo install --path crates/frost-cli
cargo install --path crates/frost-mcp
```

> crates.io publication is planned but not yet wired into the release
> pipeline. For now, install from source or the GitHub Release artifacts.

### Try without a real catalog

```bash
# Runs all checks against a synthetic table so you can see the output
frost demo
frost demo -f json
```

When you want real findings, point `frost` at a catalog. It will exit with
a clear error (exit code 2) if the catalog isn't reachable — it never falls
back to synthetic data silently.

### CLI Usage

```bash
# Show a sample report against synthetic data (no catalog needed)
frost demo

# Check a table against a local warehouse
frost check db.events --warehouse ./warehouse

# Check with specific checks only
frost check db.events --checks small_files,snapshot_bloat

# JSON output for scripting
frost check db.events -f json

# GitHub Actions annotation format (for CI)
frost check db.events -f github-actions

# Cost waste estimate
frost cost db.events --warehouse ./warehouse

# Generate fix command
frost fix db.events small_files

# List all tables
frost list --warehouse ./warehouse
```

### CLI Output

```
╔══════════════════════════════════════════════════════════════╗
║  frost · Iceberg Table Health Report                        ║
║  Table: db.events                                           ║
║  Snapshots: 120 · Data Files: 75 · Size: 7.0 GB            ║
╠══════════════════════════════════════════════════════════════╣
║                                                              ║
║  ⚠  SMALL FILES           25 files under 8MB (33.3%)        ║
║     Impact: Query planning overhead, slow reads              ║
║     Fix: Run rewrite_data_files with binpack strategy        ║
║                                                              ║
║  🔴 SNAPSHOT BLOAT         120 snapshots, oldest: 10d ago    ║
║     Impact: Metadata growth, slow planning                   ║
║     Fix: Expire snapshots older than 7 days                  ║
║                                                              ║
║  ✅ ORPHAN FILES           None detected                     ║
║  ✅ PARTITION SKEW         Balanced                          ║
║                                                              ║
║  Overall: 1 warning(s), 1 critical                           ║
╚══════════════════════════════════════════════════════════════╝
```

---

## MCP Server Setup

frost's MCP server is the primary interface — designed for Claude Code, Cursor, and any MCP-compatible agent.

### Claude Code

Add to your Claude Code MCP config (`~/.claude/claude_code_config.json`):

```json
{
  "mcpServers": {
    "frost": {
      "command": "frost-mcp",
      "args": ["--warehouse", "/path/to/warehouse"]
    }
  }
}
```

Or with a config file:

```json
{
  "mcpServers": {
    "frost": {
      "command": "frost-mcp",
      "args": ["--config", "/path/to/frost.toml"]
    }
  }
}
```

### Cursor

Add to `.cursor/mcp.json` in your project:

```json
{
  "mcpServers": {
    "frost": {
      "command": "frost-mcp",
      "args": ["--warehouse", "/path/to/warehouse"]
    }
  }
}
```

### MCP Tools

| Tool | Input | Output | Agent Use Case |
|---|---|---|---|
| `check_table` | table identifier, optional check filter | Full health report (severity, findings, costs) | "Before I write to this table, is it healthy?" |
| `check_catalog` | optional namespace filter | Summary across all tables, sorted by severity | "Which tables need attention?" |
| `check_fleet` | optional namespace filter, dormant_days | Namespace rollup, dormant tables, format-v1 tables, top offenders | "How healthy is my whole fleet?" |
| `get_fix` | table identifier, finding ID | Exact Spark SQL CALL statement + estimated scope | "Generate the compaction command and tell me how big it is" |
| `dry_run_fix` | table identifier, finding ID | Estimated scope (files/bytes/partitions) — no executable command | "Tell me the cost of fixing this before I commit" |
| `get_cost_report` | table identifier | Estimated monthly cost waste with the assumptions used to compute it | "How much are we wasting?" |
| `watch_status` | optional table, include_trend, trend_days | Watch state, recent alerts, rolling 7d/30d trend (improving/degrading/flapping) | "Is this table getting worse over time?" |

### Agent Workflow Example

An agent can autonomously diagnose and fix table health issues:

```
1. Agent calls frost.check_table("db.events")
   → Frost returns: {findings: [{id: "small_files", severity: "warning", ...}]}

2. Agent calls frost.get_fix("db.events", "small_files")
   → Frost returns: {command: "CALL catalog.system.rewrite_data_files(
       table => 'db.events', strategy => 'binpack')"}

3. Agent executes the fix via Spark SQL

4. Agent calls frost.check_table("db.events") again to verify
   → Frost returns: {findings: [], overall: {severity: "pass"}}
```

No human intervention. The agent diagnosed, fixed, and verified.

---

## Watch Mode

Monitor all tables continuously with alerts on health changes:

```bash
# Start watching (checks every 6 hours by default)
frost watch --warehouse ./warehouse

# Custom interval with Slack alerts
frost watch --interval 1h --webhook https://hooks.slack.com/services/...

# Filter to a specific namespace
frost watch --namespace production

# Check current watch status
frost watch-status
```

Watch mode:
- Stores health history in SQLite (`./frost-watch.db`)
- Detects severity changes and new/resolved findings
- Fires webhook alerts (Slack-compatible) on health regressions
- The MCP `watch_status` tool queries this state for agents

---

## CI Integration

### GitHub Actions

```yaml
name: Table Health Gate
on: [push, pull_request]

jobs:
  frost-check:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4

      - name: Install frost
        run: cargo install --path crates/frost-cli

      - name: Check table health
        run: frost check db.events --warehouse ./warehouse -f github-actions
```

frost exits with code 1 if any critical findings are detected, failing the CI step. The `github-actions` format produces `::warning` and `::error` annotations that appear inline in PR diffs.

---

## Configuration

Create a `frost.toml` in your project root (see `config/frost.example.toml`):

```toml
[catalog]
type = "filesystem"
warehouse = "./warehouse"

[thresholds]
small_file_bytes = 8_388_608      # 8 MB
max_snapshots = 100
max_snapshot_age_days = 7
partition_skew_ratio = 10.0
max_delete_files = 50
stale_table_hours = 48
max_metadata_bytes = 524_288_000  # 500 MB

[cost]
s3_storage_per_gb_month = 0.023
s3_get_request_per_1000 = 0.0004
region = "us-east-1"

[watch]
interval = "6h"
# webhook_url = "https://hooks.slack.com/services/..."
sqlite_path = "./frost-watch.db"

[output]
default_format = "pretty"
```

### Catalog Types

```toml
# AWS Glue (requires --features glue)
[catalog]
type = "glue"
region = "us-east-1"
warehouse = "s3://my-bucket/warehouse"

# Iceberg REST Catalog (Polaris, Lakekeeper, Unity, Gravitino, Nessie, etc.)
[catalog]
type = "rest"
uri = "http://localhost:8181"
warehouse = "s3://my-bucket/warehouse"
# token = "my-bearer-token"       # Optional: Bearer token for auth
# prefix = "my_catalog"           # Optional: REST catalog prefix

# Local filesystem (development)
[catalog]
type = "filesystem"
warehouse = "./warehouse"
```

---

## Architecture

```
frost/
├── crates/
│   ├── frost-core/       # Engine: checks, cost estimation, fix generation, watch
│   ├── frost-cli/        # CLI: human interface (frost check, frost watch, etc.)
│   └── frost-mcp/        # MCP server: agent interface (stdio transport)
├── config/
│   └── frost.example.toml
└── sandbox/
    ├── docker-compose.yml        # Local dev (Spark + REST catalog + MinIO)
    └── generate_pathologies.py   # Create test tables with known issues
```

All three interfaces share `frost-core`. The MCP server isn't a wrapper around the CLI — the CLI is a thin wrapper around the library that the MCP server also uses.

### Language: Rust

Single binary distribution. Fast enough to check hundreds of tables in seconds. The metadata-only design means frost never touches data files — it reads snapshot JSON, manifest lists, and manifest files to extract all health signals.

---

## Docker

```bash
# Build the MCP server image
docker build -t frost-mcp .

# Run with stdio transport
docker run -i frost-mcp --warehouse /data/warehouse

# Mount a local warehouse
docker run -i -v /path/to/warehouse:/data/warehouse frost-mcp --warehouse /data/warehouse
```

---

## Development

```bash
# Run all tests
cargo test --workspace

# Run with verbose logging
RUST_LOG=frost=debug frost check db.events --warehouse ./warehouse

# Local dev environment (Spark + Iceberg REST catalog + MinIO)
cd sandbox && docker compose up -d
docker compose exec -T spark-iceberg spark-submit /opt/sandbox/generate_pathologies.py
```

### Test Coverage

- 46 tests across the workspace
- Unit tests for individual health checks
- Integration tests with real Iceberg metadata (Avro manifests)
- MCP tool handler tests
- Watch mode state management tests

---

## License

Apache-2.0
