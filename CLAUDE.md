# CLAUDE.md

Agent-first Iceberg table health tool. MCP server, CLI, and CI gate that inspects table metadata, diagnoses operational problems, estimates cost waste, and generates fix commands. Metadata-only — never reads data files.

## Build & Test

```bash
cargo build --workspace          # Build everything
cargo test --workspace           # Run all 46 tests
cargo fmt --all -- --check       # Check formatting
cargo clippy --workspace -- -D warnings  # Lint (warnings are errors)
```

Single crate: `cargo test -p frost-core`, `cargo test -p frost-mcp`, `cargo test -p frost-cli`

Glue catalog (optional): `cargo build -p frost-core --features glue`

## Project Layout

```
crates/
  frost-core/     # Library: checks, catalog backends, cost, fix, watch, parsing
  frost-cli/      # Binary "frost": clap CLI (check, cost, fix, list, watch, watch-status)
  frost-mcp/      # Binary "frost-mcp": MCP server (stdio transport, 5 tools)
config/           # frost.example.toml
sandbox/          # Local dev environment (docker-compose + test data generator)
```

All three interfaces share frost-core. The MCP server is not a wrapper around the CLI.

## Architecture

**Catalog backends** (`frost-core/src/catalog/`): `CatalogProvider` trait with `load_table()` and `list_tables()`. Implementations: `FilesystemCatalog` (in mod.rs), `RestCatalog` (rest.rs), `GlueCatalog` (glue.rs, feature-gated). Trait methods return `Pin<Box<dyn Future<...>>>`.

**Health checks** (`frost-core/src/checks/`): `HealthCheck` trait — each check is a stateless zero-sized struct with `id()`, `name()`, `check()`. Nine checks: small_files, snapshot_bloat, orphan_files, partition_skew, delete_pressure, schema_history, metadata_size, sort_order, freshness. Registry: `checks::all_checks()`.

**Engine** (`frost-core/src/engine.rs`): Orchestrates checks against `TableMetadata` + `FrostConfig`, returns `HealthReport`.

**Parsing** (`frost-core/src/parse/`): `metadata_json.rs` parses Iceberg v1/v2 metadata JSON. `manifest.rs` parses Avro manifest files. `fixtures.rs` generates test data.

**Watch mode** (`frost-core/src/watch.rs`): SQLite-backed state with `WatchDb`. Change detection compares finding sets between runs. Webhook alerts on severity regressions.

**MCP server** (`frost-mcp/src/server.rs`): Uses `rmcp` with `#[tool_router]` macro. The macro makes tool methods private, so public `run_*()` methods delegate for testability.

## Key Patterns

- Errors use `thiserror`. Three main types: `CatalogError`, `WatchError`, `ConfigError`.
- Config is TOML-based with `#[serde(default)]` on everything. All thresholds have sensible defaults.
- `CatalogConfig` is a tagged enum: `type = "filesystem"`, `type = "rest"`, `type = "glue"`.
- MCP tool handlers return JSON strings, converting errors to `{"error": "..."}` JSON.
- Async runtime is tokio with `features = ["full"]`.
- Logging via `tracing`. MCP logs to stderr (stdout is the protocol channel).

## Testing

Tests use real Iceberg metadata fixtures in temp dirs — no mocking. Pattern:
1. `fixture_helpers` create table layouts with known pathologies (small files, bloated snapshots, etc.)
2. `FilesystemCatalog` loads them
3. Engine runs checks
4. Assert on findings

Fixture helpers exist in two places: `frost-core/tests/fixture_helpers.rs` and `frost-mcp/tests/fixture_helpers.rs`. Unit test helper `make_test_metadata()` is in `frost-core/src/test_helpers.rs`.

Async tests use `#[tokio::test]`. SQLite tests use `WatchDb::open_in_memory()`.

## CI

GitHub Actions (`.github/workflows/ci.yml`): four parallel jobs — check, test, fmt, clippy. Runs on push to `main` or `claude/**` branches, and PRs to `main`. `RUSTFLAGS=-D warnings` is set globally.

## Style

- Rust edition 2024, resolver v2
- Default rustfmt (no rustfmt.toml)
- Default clippy with `-D warnings` (no clippy.toml)
- Indent: 4 spaces for Rust, 2 for TOML/YAML
