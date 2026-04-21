# Contributing to frost

Thanks for your interest in improving frost. This document covers the
workflow for reporting issues, submitting pull requests, and finding your
way around the codebase.

## Reporting issues

- Use the issue templates in `.github/ISSUE_TEMPLATE/` for bug reports and
  feature requests.
- For bugs, include the catalog type (REST/Glue/Filesystem), a minimal
  repro, and the full output of `RUST_LOG=frost=debug frost check ...`.
- Security issues: see `SECURITY.md` — do not open public issues.

## Pull requests

1. Fork and create a feature branch off `main`.
2. Run `cargo fmt --all`, `cargo clippy --workspace -- -D warnings`, and
   `cargo test --workspace` locally. CI enforces all three.
3. Keep PRs focused. New checks, new catalog backends, and refactors each
   belong in separate PRs.
4. Update `CHANGELOG.md` under `## [Unreleased]` with a one-line summary
   of your change in the appropriate section.
5. If you change public API, update the README and any inline rustdoc.

## Project layout

See `CLAUDE.md` for a concise architectural tour. Quick version:

- `crates/frost-core` — engine, checks, catalog trait, parsing, watch mode
- `crates/frost-cli` — `frost` CLI binary
- `crates/frost-mcp` — `frost-mcp` MCP server binary
- `sandbox/` — docker-compose stack for local E2E testing

## Adding a new health check

1. Create `crates/frost-core/src/checks/<name>.rs` implementing `HealthCheck`.
2. Register it in `crates/frost-core/src/checks/mod.rs::all_checks()`.
3. Add any new thresholds to `config::Thresholds` with a sensible default.
4. Add unit tests using `make_test_metadata()` from `test_helpers.rs`.
5. If the issue is fixable, add a match arm in `fix::generate_fix`.
6. Document the check in `README.md`'s check table.

## Adding a new catalog backend

1. Create `crates/frost-core/src/catalog/<name>.rs` implementing
   `CatalogProvider`.
2. Add a variant to `config::CatalogConfig` (tagged enum, `type = "..."`).
3. Wire it into `catalog::from_config`.
4. Add an integration test in `crates/frost-core/tests/`.
5. Update the README's catalog types section and `config/frost.example.toml`.

## Running the local sandbox

```bash
cd sandbox
docker compose up -d
docker compose exec spark-iceberg spark-submit /opt/sandbox/generate_pathologies.py
frost check demo.small_files --config sandbox/frost.toml
```

## Release process

Releases are cut from `main`:

1. Bump version in `Cargo.toml` (workspace `version`).
2. Move `## [Unreleased]` entries to a new `## [x.y.z] — YYYY-MM-DD` section.
3. Tag: `git tag -s vX.Y.Z -m "vX.Y.Z"` and push.
4. The `release` workflow builds binaries via `cargo-dist`, publishes a
   GHCR Docker image, and (for maintainers with the token) runs
   `cargo publish` for each crate in dependency order.

## License

By contributing, you agree that your contributions will be licensed under
the Apache License 2.0. See `LICENSE`.
