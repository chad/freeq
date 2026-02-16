FROM rust:1.86-slim AS builder
RUN apt-get update && apt-get install -y pkg-config libssl-dev && rm -rf /var/lib/apt/lists/*
WORKDIR /build
COPY Cargo.toml Cargo.lock ./
COPY freeq-sdk/ freeq-sdk/
COPY freeq-server/ freeq-server/
COPY freeq-tui/ freeq-tui/
RUN cargo build --release --package freeq-server

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y ca-certificates && rm -rf /var/lib/apt/lists/*
RUN mkdir -p /app/data
COPY --from=builder /build/target/release/freeq-server /app/freeq-server
WORKDIR /app
EXPOSE 8080
CMD ["/app/freeq-server", "--listen-addr", "127.0.0.1:16667", "--web-addr", "0.0.0.0:8080", "--server-name", "irc.freeq.at", "--db-path", "/app/data/freeq.db", "--data-dir", "/app/data", "--motd", "Welcome to freeq — IRC with AT Protocol identity. https://freeq.at"]
