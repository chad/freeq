FROM rust:1.89-slim AS builder
RUN apt-get update && apt-get install -y pkg-config libssl-dev && rm -rf /var/lib/apt/lists/*
WORKDIR /build
# The deploy script stages exactly the workspace members Cargo.toml
# references (derived from the members block, so it can't go stale).
# Copy the whole staged context rather than enumerating members here —
# a hardcoded list silently breaks the build whenever a crate is added.
COPY . .
RUN cargo build --release --package freeq-server

# Runtime base must match the builder's GLIBC — `rust:1.89-slim` is
# Debian trixie (GLIBC 2.41), so we can't drop the binary onto bookworm
# (GLIBC 2.36) or it crashes at startup with "GLIBC_2.38 not found".
FROM debian:trixie-slim
RUN apt-get update && apt-get install -y ca-certificates && rm -rf /var/lib/apt/lists/*
RUN mkdir -p /app/data
COPY --from=builder /build/target/release/freeq-server /app/freeq-server
WORKDIR /app
EXPOSE 8080
CMD ["/app/freeq-server", "--listen-addr", "127.0.0.1:16667", "--web-addr", "0.0.0.0:8080", "--server-name", "irc.freeq.at", "--db-path", "/app/data/freeq.db", "--data-dir", "/app/data", "--motd", "Welcome to freeq — IRC with AT Protocol identity. https://freeq.at"]
