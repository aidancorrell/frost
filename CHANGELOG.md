# Changelog

All notable changes to frost are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and this project
adheres to [Semantic Versioning](https://semver.org/).

## [Unreleased]

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

### Fixed
- REST catalog's `load_table` returned metadata without data files on all
  catalogs — see "Added" above.
- Glue catalog would fail to download metadata from any private bucket.

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
