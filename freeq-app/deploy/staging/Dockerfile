# freeq staging — IRC server + web client in one image
#
# Uses cargo-chef for dependency caching: changing source code only rebuilds
# freeq crates (~30s), not all 1300+ transitive dependencies (~20min).

# ── Chef: compute dependency recipe ──
FROM rust:1.93-slim AS chef
RUN cargo install cargo-chef --locked
RUN apt-get update && apt-get install -y pkg-config libssl-dev cmake clang libasound2-dev curl && rm -rf /var/lib/apt/lists/*
RUN curl -fsSL https://deb.nodesource.com/setup_20.x | bash - && apt-get install -y nodejs && rm -rf /var/lib/apt/lists/*
WORKDIR /build

FROM chef AS planner
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

FROM chef AS deps
COPY --from=planner /build/recipe.json recipe.json
RUN cargo chef cook --release --recipe-path recipe.json --package freeq-server 2>/dev/null || true
RUN cargo chef cook --release --recipe-path recipe.json --package freeq-server --features av-native 2>/dev/null || true

FROM deps AS builder
COPY . .
RUN cargo build --release --package freeq-server --features av-native 2>/dev/null \
    || cargo build --release --package freeq-server

FROM node:20-slim AS web-builder
WORKDIR /build
# Build @freeq/sdk first — freeq-app depends on its compiled dist
COPY freeq-sdk-js/ ./freeq-sdk-js/
RUN cd freeq-sdk-js && npm ci --silent && npm run build
# Then build the web app
COPY web-client/package.json web-client/package-lock.json ./web-client/
RUN cd web-client && npm ci --ignore-scripts
COPY web-client/ ./web-client/
RUN cd web-client && npx vite build && node -e "const fs=require('fs'),h=Date.now().toString(36);let s=fs.readFileSync('dist/sw.js','utf8');s=s.replace('__BUILD_HASH__',h);fs.writeFileSync('dist/sw.js',s);console.log('SW cache:',h)"

FROM debian:trixie-slim
RUN apt-get update && apt-get install -y ca-certificates && rm -rf /var/lib/apt/lists/*
RUN mkdir -p /app/data /app/web
COPY --from=builder /build/target/release/freeq-server /app/freeq-server
COPY --from=web-builder /build/web-client/dist /app/web
WORKDIR /app
EXPOSE 8080 4443
CMD /app/freeq-server \
  --listen-addr 127.0.0.1:16667 \
  --web-addr 0.0.0.0:${PORT:-8080} \
  --web-static-dir /app/web \
  --server-name staging.freeq.at \
  --db-path /app/data/freeq.db \
  --data-dir /app/data \
  --iroh \
  --motd "freeq staging — AV with iroh-live"
