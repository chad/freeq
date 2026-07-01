# ── Build stage ──
FROM rust:1.89-slim-bookworm AS builder

RUN apt-get update && apt-get install -y pkg-config libssl-dev && rm -rf /var/lib/apt/lists/*

WORKDIR /src
COPY . .

RUN cargo build --release -p freeq-server -p freeq-auth-broker

# ── Web client build ──
FROM node:20-slim AS web-builder

WORKDIR /src
COPY freeq-sdk-js/ freeq-sdk-js/
COPY freeq-app/ freeq-app/
WORKDIR /src/freeq-app
RUN npm ci --ignore-scripts && npm run build

# ── Runtime ──
FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y ca-certificates && rm -rf /var/lib/apt/lists/*
RUN useradd -r -s /bin/false freeq

WORKDIR /app

COPY --from=builder /src/target/release/freeq-server /usr/local/bin/
COPY --from=builder /src/target/release/freeq-auth-broker /usr/local/bin/
COPY --from=web-builder /src/freeq-app/dist /app/web

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
