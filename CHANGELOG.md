# Changelog

All notable changes to frost are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and this project
adheres to [Semantic Versioning](https://semver.org/).

## [Unreleased]

## [0.3.0] — Depth

This release closes the gap between "frost counts things" and "frost
diagnoses things." Adds six new health checks driven by manifest fields
that were previously parsed and discarded, rewrites three shallow
checks to measure (bytes/rows) instead of count, adds rolling trend
detection to watch mode, fix-scope estimation, fleet-level signals,
and a derived cost model. No catalog-backend or transport changes —
this is an engine-depth release.

### Added
- **6 new health checks**: `format_v1` (flags Iceberg v1 tables that
  should migrate to v2), `properties_drift` (`write.target-file-size-bytes`
  declared vs observed median), `partition_spec_evolution` (retired-spec
  churn and files left under old specs), `sort_compliance` (uses
  manifest `sort_order_id` to verify files honor the declared order),
  `stats_coverage` (% of files with column statistics in their
  manifest entries), `branch_health` (per-branch/tag staleness and
  dangling-ref detection).
- `TableMetadata` now captures `format_version`, `table_uuid`,
  `properties` (e.g. `write.target-file-size-bytes`,
  `write.distribution-mode`), `partition_specs` history,
  `sort_orders` history, `refs` (Iceberg v2 named branches and tags),
  and `manifest_stats` (count, total/median/max manifest size).
- `Snapshot` now carries `parent_snapshot_id`, `operation`, and
  `schema_id`.
- `DataFile` now carries `column_sizes`, `value_counts`,
  `null_value_counts`, `sort_order_id`, and `spec_id` extracted from
  manifest entries.
- `DeleteFile` now carries `equality_ids` for equality deletes.
- `frost-core::fleet` module: namespace rollup, dormant-table
  detection (no commits in N days), unpartitioned-table scan,
  format-v1 fleet scan, top-N offenders.
- New CLI commands: `frost fleet [namespace]` and `frost watch-trends
  <table>`.
- New MCP tools: `check_fleet`, `dry_run_fix`. `watch_status` now
  optionally returns rolling trend signals
  (`improving`/`degrading`/`flapping`/`stable`).
- Watch mode: `WatchDb::compute_trend` computes rolling 7d/30d trend
  classifications; alert pipeline now suppresses duplicate alerts
  within a 1-hour cooldown to reduce flap noise.
- `FixCommand` now carries a `FixScope` (estimated_files,
  estimated_bytes, estimated_partitions, estimated_snapshots_expired)
  so agents can reason about the cost of running a fix before
  committing.
- Fixes for the 5 new findings (`format_v1`, `properties_drift`,
  `partition_spec_evolution`, `sort_compliance`, `stats_coverage`).
- Five new tunable thresholds: `max_partition_specs`,
  `min_sort_compliance_pct`, `min_stats_coverage_pct`,
  `stale_branch_days`, `target_file_size_drift_pct`.
- Three new `[cost]` knobs to replace hardcoded heuristics:
  `queries_per_day`, `compute_cost_per_cpu_hour`,
  `avg_bytes_scanned_per_query`.

### Changed
- `partition_skew` now measures bytes and rows alongside file counts
  and reports max/median plus p95/p99 distribution percentiles. The
  driving dimension (bytes/rows/files) is surfaced in the message so
  the operator knows why a partition is hot.
- `delete_pressure` weights equality deletes ~5× more than position
  deletes (reflecting their full-scan read cost) and surfaces the
  fraction of table rows shadowed by pending deletes. Critical
  threshold now also fires when row-shadow exceeds 25%.
- `small_files` now reports bytes-trapped (the volume compaction will
  read+rewrite) and the top partition hotspots, not just file counts.
- `orphan_files` fix command now includes `older_than = TIMESTAMP
  '<3 days ago>'` by default to protect against deleting files from
  in-flight commits.
- Cost model:
  - `snapshot_bloat` is now derived from real `metadata_size_bytes`
    pro-rata, not the magic 4 KB / 10 MB heuristic.
  - New `manifest_planning_gets` line item for the per-query GETs
    incurred by manifest count.
  - New `delete_merge_overhead` line item that models scan-time CPU
    cost from deletes (position 0.1%/file, equality 0.5%/file × CPU
    cost × queries-per-day).
  - `CostReport` now embeds an `assumptions` block so the reader can
    sanity-check the dollar figure against their workload.
- MCP `watch_status` accepts new optional params `include_trend` and
  `trend_days`.

### Notes
- The Avro manifest extractor is permissive of older writers: column
  stats and `sort_order_id` are optional. Tables written by
  Spark <3.5 / Trino <380 will pass `sort_compliance` and
  `stats_coverage` only if they were rewritten by a recent engine.
- The healthy-table integration test now exempts `stats_coverage` and
  `sort_compliance` because the test fixture writer (intentionally)
  doesn't emit those fields. Both checks have dedicated unit tests.


## [0.2.0] — 2026-05-02

### Added
- `LICENSE`, `CONTRIBUTING.md`, `CHANGELOG.md`, `SECURITY.md`,
  `CODE_OF_CONDUCT.md` for OSS hygiene.
- GitHub issue and PR templates under `.github/`.
- `ROADMAP.md` describing the path to a viable OSS release.
- crates.io publication metadata (description, keywords, categories,
  readme, homepage) on all three crates.
- Minimal `ObjectStore` abstraction (`frost-core::object_store`) with
  local filesystem and S3 implementations (SigV4 via `aws-sdk-s3`).
- REST catalog now fetches manifest lists and manifests through
  `ObjectStore` instead of silently skipping them, so small_files,
  partition_skew, delete_pressure, and orphan_files finally work on
  Polaris/Lakekeeper/Unity/Nessie.
- `frost demo` subcommand for generating a sample health report against
  synthetic data.
- `cargo-dist` configuration for pre-built release binaries
  (Linux x86_64/aarch64, macOS universal, Windows x86_64).
- `.github/workflows/release.yml` publishes binaries to GitHub Releases
  and a container image to `ghcr.io/aidancorrell/frost` on tag push.
- `.github/workflows/e2e.yml` spins up the sandbox REST catalog
  and runs `frost check` against a table with known pathologies.

### Changed
- Glue catalog's S3 downloads now use `aws-sdk-s3` with the standard
  credential provider chain instead of unauthenticated HTTPS; private
  buckets work now.
- `frost check` no longer silently falls back to synthetic demo data when
  a catalog load fails. It reports the error and exits non-zero. Use
  `frost demo` if you want the synthetic report.
- S3 client now uses path-style addressing, so MinIO and other
  S3-compatible stores (Ceph, Wasabi, R2 with path-style) work without
  custom DNS. Real AWS S3 still works — path-style is universally
  supported there.
- E2E workflow no longer runs on a nightly cron; kept on-demand
  (`workflow_dispatch`) and on-push (catalog/object-store path-scoped)
  triggers to avoid burning Actions minutes on a project with no users.

### Fixed
- REST catalog's `load_table` returned metadata without data files on all
  catalogs — see "Added" above.
- Glue catalog would fail to download metadata from any private bucket.
- `frost check -f json` produced unparseable output when redirected
  with `>`. The `tracing` subscriber defaulted to stdout and prepended
  ANSI-colored log lines to the JSON document. Logs now go to stderr,
  so `frost check ... -f json > out.json` produces a clean JSON file.

### Known follow-ups (tracked in `ROADMAP.md`)
- Hive Metastore catalog (Phase 7).
- S3 Tables catalog (Phase 7).
- MCP resources and prompts, batch endpoints, dry-run fix (Phase 8).
- New checks: table-property drift, format-v1 tables, manifest size
  distribution, missing statistics, age-weighted orphans (Phase 9).
- `criterion` benchmarks, OpenTelemetry, structured JSON logs (Phase 9).
- mdBook docs site, hosted demo, MCP registry listing (Phase 10).

## [0.1.0] — 2026-04-20

Initial prototype. Three crates, nine checks, three catalog backends
(Filesystem, REST, Glue behind a feature flag), MCP server with five
tools, CLI with six commands, SQLite-backed watch mode, 46 tests.
