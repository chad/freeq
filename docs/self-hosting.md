# Self-Hosting Guide

Run your own freeq server with TLS, the web client, and optional features.

## Quick start

```bash
git clone https://github.com/chad/freeq
cd freeq
cargo build --release -p freeq-server

# Start with defaults (port 6667, no TLS)
./target/release/freeq-server --bind 0.0.0.0:6667
```

## With TLS

```bash
./target/release/freeq-server \
  --bind 0.0.0.0:6667 \
  --tls-bind 0.0.0.0:6697 \
  --tls-cert /path/to/cert.pem \
  --tls-key /path/to/key.pem
```

## With the web client

```bash
cd freeq-app && npm install && npm run build && cd ..

./target/release/freeq-server \
  --bind 0.0.0.0:6667 \
  --web-bind 0.0.0.0:8080 \
  --web-static-dir freeq-app/dist
```

The web client will be served at `http://localhost:8080` with WebSocket IRC at `/irc`.

## With nginx (production)

```nginx
server {
    listen 443 ssl http2;
    server_name irc.example.com;

    ssl_certificate /etc/letsencrypt/live/irc.example.com/fullchain.pem;
    ssl_certificate_key /etc/letsencrypt/live/irc.example.com/privkey.pem;

    location /irc {
        proxy_pass http://127.0.0.1:8080;
        proxy_http_version 1.1;
        proxy_set_header Upgrade $http_upgrade;
        proxy_set_header Connection "upgrade";
        proxy_set_header Host $host;
        proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
    }

    location / {
        proxy_pass http://127.0.0.1:8080;
        proxy_set_header Host $host;
        proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
    }
}
```

## systemd service

```ini
[Unit]
Description=freeq IRC server
After=network.target

[Service]
Type=simple
User=freeq
WorkingDirectory=/opt/freeq
ExecStart=/opt/freeq/freeq-server \
  --bind 0.0.0.0:6667 \
  --tls-bind 0.0.0.0:6697 \
  --tls-cert /etc/letsencrypt/live/irc.example.com/fullchain.pem \
  --tls-key /etc/letsencrypt/live/irc.example.com/privkey.pem \
  --web-bind 127.0.0.1:8080 \
  --web-static-dir /opt/freeq/freeq-app/dist \
  --server-name irc.example.com
Restart=always
RestartSec=5

[Install]
WantedBy=multi-user.target
```

## Environment variables

| Variable | Purpose |
|---|---|
| `GITHUB_CLIENT_ID` | GitHub OAuth for credential verifier |
| `GITHUB_CLIENT_SECRET` | GitHub OAuth secret |
| `BROKER_SHARED_SECRET` | HMAC secret shared with auth broker |
| `OPER_DIDS` | Comma-separated DIDs for server operators |
| `FREEQ_LOG_JSON` | Set to `1` for structured JSON logging |

## Data files

| File | Purpose |
|---|---|
| `irc.db` | Message history, channels, user data (SQLite) |
| `irc-policy.db` | Policy rules and credentials (SQLite) |
| `msg-signing-key.secret` | Server message signing key (ed25519) |
| `verifier-signing-key.secret` | Credential verifier signing key |
| `db-encryption-key.secret` | Database encryption-at-rest key |

All key files are generated automatically on first run.

## Encryption at rest

Message text is encrypted with AES-256-GCM before writing to SQLite. The key is stored in `db-encryption-key.secret`. Messages are transparently decrypted on read.
