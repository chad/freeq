FROM rust:1.89-slim AS builder
RUN apt-get update && apt-get install -y pkg-config libssl-dev && rm -rf /var/lib/apt/lists/*
WORKDIR /build
COPY Cargo.toml Cargo.lock ./
# Workspace members referenced by Cargo.toml — cargo must resolve them all
# even when only building one. The deploy script copies whichever exist.
COPY freeq-sdk/ freeq-sdk/
COPY freeq-server/ freeq-server/
COPY freeq-tui/ freeq-tui/
COPY freeq-auth-broker/ freeq-auth-broker/
COPY freeq-bots/ freeq-bots/
COPY freeq-bot-id/ freeq-bot-id/
COPY freeq-sdk-ffi/ freeq-sdk-ffi/
COPY freeq-windows-core/ freeq-windows-core/
COPY freeq-av-client/ freeq-av-client/
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
