# Security policy

## Reporting a vulnerability

If you believe you've found a security issue in frost, **do not open a
public GitHub issue.** Email the maintainers via GitHub's private
vulnerability reporting feature:

1. Go to <https://github.com/aidancorrell/frost/security/advisories/new>.
2. Describe the issue, including a minimal repro if possible.
3. We will acknowledge within 3 business days and target a patched
   release within 30 days of confirmation.

## Scope

frost is a metadata-only tool. It reads Iceberg metadata JSON, manifest
lists, and manifests — it never reads, writes, or deletes data files.
Security concerns typically fall into one of these areas:

- **Credential handling** — how AWS / catalog credentials are resolved
  and passed through to SDKs.
- **Path traversal** — malicious metadata pointing at paths outside the
  table's warehouse.
- **HTTP/TLS** — how REST catalog clients validate certificates and
  handle redirects.
- **Generated fix commands** — whether table identifiers are properly
  escaped in the Spark SQL / Trino / Flink commands `frost` emits.

Non-security issues (false-positive findings, broken checks, CLI
ergonomics) belong in the regular issue tracker.

## Supported versions

Until frost reaches 1.0, security fixes are backported only to the most
recent minor release.
