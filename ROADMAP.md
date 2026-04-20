# frost roadmap

Assessment of what stands between frost (v0.1.0) and being a viable open-source
Iceberg health tool, plus a sequenced plan to get there.

## Where we are

v0.1.0 is a well-architected prototype:

- 3 crates (core / cli / mcp), shared engine, trait-based catalogs and checks
- 9 health checks (small_files, snapshot_bloat, orphan_files, partition_skew,
  delete_pressure, schema_history, metadata_size, sort_order, freshness)
- 3 catalog backends (Filesystem, REST, Glue)
- MCP server with 5 tools, CLI with 6 commands, watch mode with SQLite state
- 46 tests, CI (check/test/fmt/clippy), Dockerfile, docker-compose sandbox
- Metadata-only design is defensible and differentiated

The bones are good. The gaps are in correctness on real catalogs, ecosystem
coverage, project hygiene, and release engineering.

## Critical gaps blocking real use

These are the issues that would bite a user in the first 15 minutes:

1. **REST catalog skips manifests.** `RestCatalog::load_table` stops after
   parsing the metadata JSON; it never fetches manifest lists or manifests from
   object storage. That silently disables small_files, partition_skew,
   delete_pressure, and orphan_files for every REST user — and REST (Polaris,
   Lakekeeper, Unity, Nessie, Gravitino) is the majority of real deployments.
   The comment at `rest.rs:125-129` acknowledges this; it needs to be fixed,
   not documented.

2. **Glue S3 fetch uses unauthenticated HTTPS.** `download_s3_bytes` in
   `glue.rs:290-293` hits `https://<bucket>.s3.amazonaws.com/<key>` with no
   signing — works only on public buckets, which is nobody's production
   warehouse. Needs `aws_sdk_s3` with credential chain.

3. **No object-store support for the filesystem catalog.** The `Filesystem`
   variant only reads local paths. Teams running their own warehouse on S3 and
   pointing at a path-based layout (no catalog server) cannot use frost.

4. **Demo-data fallback is dangerous.** `load_metadata` in `frost-cli/main.rs`
   silently falls back to a hardcoded fake table when the catalog errors. An
   agent calling `check_table` would get convincing-looking findings from
   synthetic data. This must fail loud or be removed.

5. **No LICENSE file.** Apache-2.0 is declared in README and Cargo.toml but
   the repo has no `LICENSE` file. Blocks packaging, compliance review, and
   crates.io publish.

## Viability gaps

Important but not immediately blocking:

- Not published to crates.io; no pre-built binaries in GitHub Releases; no
  published Docker image on Docker Hub / GHCR.
- No CONTRIBUTING.md, CHANGELOG.md, SECURITY.md, CODE_OF_CONDUCT.md, issue or
  PR templates.
- No end-to-end CI: the `sandbox/` compose stack exists but is not wired into
  GitHub Actions, so the REST path is never tested against a real catalog.
- No benchmarks — README claims "checking 100K-file tables takes seconds,"
  but there is no harness that proves this.
- No Hive Metastore support (still the default in EMR and a lot of on-prem
  deployments).
- No S3 Tables support (AWS's managed Iceberg service, growing quickly).
- Unity Catalog REST works in theory but the Databricks OAuth/PAT flow is not
  documented or tested.
- Watch mode only supports webhooks — no PagerDuty, OpsGenie, or email sinks;
  no OpenTelemetry/Prometheus metrics; no structured JSON logging.

## Depth and polish gaps

- MCP surface is tools-only — no resources (tables as browseable resources),
  no prompts (canned diagnostic workflows), no sampling.
- No concise/verbose output modes for agents (big JSON blobs chew context
  budget).
- No batch endpoints: agents must loop over `check_table` themselves.
- No `get_fix` dialects beyond generic Spark SQL (no Trino, no Flink, no
  direct REST catalog calls for snapshot expiration).
- Check coverage misses a few common problems: table properties drift (e.g.
  `write.target-file-size-bytes` set but not honored), format-v1 tables,
  excessive partition spec evolution, manifest size distribution, missing
  column statistics / puffin files, ancient orphans (age-weighted).
- No cost model for query-plan-time S3 GETs; only storage and request counts.

## Plan

Sequenced in phases. Each phase should land as its own PR stack and tag a
release.

### Phase 6 — Launch ready (0.2.0)

Goal: a user can `cargo install frost-cli`, point at a real REST catalog, and
get correct findings.

1. **Fix REST manifest fetching.** Add an `ObjectStore` trait in `frost-core`
   with S3/GCS/Azure/HTTP impls (use the `object_store` crate or hand-rolled
   minimal S3 via `aws_sdk_s3`). REST catalog resolves `s3://...` manifest
   paths through it.
2. **Fix Glue S3 auth.** Swap `reqwest::get` for `aws_sdk_s3::Client` using
   the same credential chain as the Glue client.
3. **Kill the demo-data fallback.** Replace with a clear error and a
   `--demo` flag (or `frost demo` subcommand) if we want to keep the
   experience for first-time users.
4. **Add LICENSE, CONTRIBUTING, CHANGELOG, SECURITY, CODE_OF_CONDUCT.**
   Issue and PR templates in `.github/`.
5. **Publish to crates.io.** Fill out `description`, `keywords`, `categories`,
   `readme`, `homepage`, `documentation` in each `Cargo.toml`.
6. **Pre-built binaries via `cargo-dist`.** Linux x86_64/aarch64,
   macOS universal, Windows x86_64. Attach to GitHub Releases.
7. **Publish Docker image** to GHCR on tag push.
8. **E2E CI job** that spins up the `sandbox/` compose stack, creates a
   pathological table via `generate_pathologies.py`, and runs `frost check`
   against the REST catalog. Flip it on as a required check.

### Phase 7 — Ecosystem reach (0.3.0)

Goal: cover the catalogs people actually run.

1. **Hive Metastore backend** — Thrift client, `metadata_location` lookup,
   same object-store fetch as REST.
2. **S3 Tables backend** — AWS's managed Iceberg. Uses Glue-adjacent APIs
   plus SigV4'd S3 for metadata.
3. **Unity Catalog auth recipes** — document Databricks PAT + OAuth2 flow,
   test against Unity's Iceberg REST endpoint.
4. **Filesystem catalog over S3** — let `warehouse = "s3://..."` just work,
   routing through the object-store abstraction.
5. **Nessie/Polaris/Lakekeeper smoke tests** — add a matrix E2E job.

### Phase 8 — Agent experience (0.4.0)

Goal: make frost the obvious choice for AI agents maintaining Iceberg tables.

1. **MCP resources** — expose tables as browseable resources
   (`iceberg://catalog/db.events`), so agents can list and read without
   calling a tool.
2. **MCP prompts** — canned diagnostic workflows ("diagnose write slowdown",
   "pre-flight a compaction"), each a sequence of tool calls the agent can
   execute.
3. **Concise vs verbose output modes** on every tool to manage context
   budget. Default concise for MCP.
4. **Batch endpoints** — `check_tables` (plural) and `get_fixes` so agents
   don't loop.
5. **`dry_run_fix` tool** — show what would change (estimated files rewritten,
   estimated snapshots expired) without executing.
6. **Engine dialects for `generate_fix`** — Spark (current), Trino, Flink,
   and direct REST catalog calls where possible.

### Phase 9 — Depth (0.5.0)

Goal: catch the problems that managed warehouses catch.

1. **New checks:** table-property drift, format-v1 tables,
   partition-spec-evolution churn, manifest size distribution,
   missing/stale statistics (puffin files), age-weighted orphans,
   delete-row-ratio (not just file count).
2. **Benchmarks** via `criterion` — real 100k-file metadata fixtures,
   budget regressions in CI.
3. **OpenTelemetry + Prometheus metrics** from watch mode.
4. **Structured JSON logging** mode for production deploys.
5. **Alert sinks:** PagerDuty, OpsGenie, email (SMTP), Microsoft Teams in
   addition to Slack-compatible webhooks.

### Phase 10 — Growth (0.6.0+)

Goal: discoverability and adoption.

1. **Docs site** (mdBook on GitHub Pages): architecture, every check with
   impact/fix/sample, catalog setup guides, agent integration cookbook.
2. **Screencast / GIF** in README showing CLI + MCP flow end to end.
3. **Hosted demo** — a public read-only warehouse with pre-baked
   pathologies, linked from the README.
4. **Listings:** MCP server registries, Awesome-Iceberg, Awesome-MCP.
5. **Write-ups** on the cost model, the metadata-only design decision,
   and agent workflows.
6. **Homebrew tap / Scoop bucket** for non-Rust users.

## Sequencing notes

- Phases 6 and 7 are strictly ordered — ecosystem reach depends on
  `ObjectStore`.
- Phase 8 can start in parallel with 7 once 6 lands.
- Phase 9 can start anytime after 6; benchmarks should land with 6 if
  possible so performance claims are real on launch.
- Phase 10 should wait for 8, so first impressions show off agent UX.

## Definition of "viable OSS tool"

Measured by: (1) `cargo install frost-cli` works, (2) all four major
catalogs (Filesystem over S3, REST, Glue, Hive) give correct findings on a
real table with private-bucket credentials, (3) LICENSE + release process
+ issue templates in place, (4) docs site live, (5) published on crates.io
with pre-built binaries. Phases 6 and 7 close this. Everything after is
quality of life.
