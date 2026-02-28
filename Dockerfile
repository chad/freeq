# ── Build stage ──
FROM rust:1.85-slim-bookworm AS builder

RUN apt-get update && apt-get install -y pkg-config libssl-dev && rm -rf /var/lib/apt/lists/*

WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY freeq-server/ freeq-server/
COPY freeq-sdk/ freeq-sdk/
COPY freeq-sdk-ffi/ freeq-sdk-ffi/
COPY freeq-tui/ freeq-tui/
COPY freeq-bots/ freeq-bots/
COPY freeq-auth-broker/ freeq-auth-broker/

RUN cargo build --release -p freeq-server -p freeq-auth-broker

# ── Web client build ──
FROM node:20-slim AS web-builder

WORKDIR /app
COPY freeq-app/package.json freeq-app/package-lock.json ./
RUN npm ci --ignore-scripts
COPY freeq-app/ ./
RUN npm run build

# ── Runtime ──
FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y ca-certificates && rm -rf /var/lib/apt/lists/*
RUN useradd -r -s /bin/false freeq

WORKDIR /app

COPY --from=builder /src/target/release/freeq-server /usr/local/bin/
COPY --from=builder /src/target/release/freeq-auth-broker /usr/local/bin/
COPY --from=web-builder /app/dist /app/web

RUN mkdir -p /data && chown freeq:freeq /data
VOLUME /data
USER freeq

ENV RUST_LOG=info

EXPOSE 6667 6697 8080

ENTRYPOINT ["freeq-server"]
CMD [ \
  "--bind", "0.0.0.0:6667", \
  "--web-addr", "0.0.0.0:8080", \
  "--web-static-dir", "/app/web", \
  "--db-path", "/data/irc.db", \
  "--data-dir", "/data" \
]
