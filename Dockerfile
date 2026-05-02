# Multi-stage build for the frost MCP server.
# Produces a minimal image with just the frost-mcp binary.

FROM rust:1.91-bookworm AS builder

WORKDIR /build

# Copy manifests first for layer caching.
COPY Cargo.toml Cargo.lock ./
COPY crates/frost-core/Cargo.toml crates/frost-core/Cargo.toml
COPY crates/frost-cli/Cargo.toml crates/frost-cli/Cargo.toml
COPY crates/frost-mcp/Cargo.toml crates/frost-mcp/Cargo.toml

# Create stub source files so cargo can resolve the workspace.
RUN mkdir -p crates/frost-core/src && echo "pub fn stub() {}" > crates/frost-core/src/lib.rs && \
    mkdir -p crates/frost-cli/src && echo "fn main() {}" > crates/frost-cli/src/main.rs && \
    mkdir -p crates/frost-mcp/src && echo "fn main() {}" > crates/frost-mcp/src/main.rs && \
    echo "pub mod server; pub mod tools;" > crates/frost-mcp/src/lib.rs && \
    touch crates/frost-mcp/src/server.rs crates/frost-mcp/src/tools.rs

# Pre-build dependencies (cached layer).
RUN cargo build --release -p frost-mcp 2>/dev/null || true

# Copy real source and build.
COPY crates/ crates/
COPY config/ config/
RUN cargo build --release -p frost-mcp

# Runtime image.
FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates && \
    rm -rf /var/lib/apt/lists/*

COPY --from=builder /build/target/release/frost-mcp /usr/local/bin/frost-mcp

# Default to stdio transport.
ENTRYPOINT ["frost-mcp"]
CMD ["--transport", "stdio"]
