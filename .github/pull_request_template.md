<!--
Thanks for contributing to frost! A few notes:

- Keep PRs focused. One check, one catalog, one refactor per PR.
- Update CHANGELOG.md under `## [Unreleased]` if your change is user-visible.
- CI will run `cargo fmt --check`, `cargo clippy -D warnings`, and
  `cargo test --workspace`. All three must pass.
-->

## Summary

<!-- What changed and why. -->

## Test plan

- [ ] `cargo fmt --all -- --check`
- [ ] `cargo clippy --workspace -- -D warnings`
- [ ] `cargo test --workspace`
- [ ] Manual verification (describe below)

<!-- Describe any manual testing: which catalog, what table, what command. -->

## Checklist

- [ ] CHANGELOG.md updated (if user-visible)
- [ ] README.md updated (if public API or check list changed)
- [ ] New tests added (unit or integration)
- [ ] No `TODO` / `FIXME` left behind without an issue reference
