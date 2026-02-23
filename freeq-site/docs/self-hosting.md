# Self-Hosting Guide

Run your own freeq server with full AT Protocol authentication, web client, and optional federation.

## Minimum setup

```bash
git clone https://github.com/chad/freeq
cd freeq
cargo build --release --bin freeq-server

./target/release/freeq-server \
  --listen-addr 0.0.0.0:6667 \
  --web-addr 0.0.0.0:8080 \
  --db-path /var/lib/freeq/freeq.db \
  --server-name irc.yourdomain.com
```

This gives you:
- IRC on port 6667
- WebSocket + REST API + web client on port 8080
- Persistent message history in SQLite

## Full production setup

### Build web client

```bash
cd freeq-app
npm install
npm run build
cd ..
```

### TLS (recommended)

```bash
./target/release/freeq-server \
  --listen-addr 0.0.0.0:6667 \
  --tls-listen-addr 0.0.0.0:6697 \
  --tls-cert /etc/letsencrypt/live/irc.yourdomain.com/fullchain.pem \
  --tls-key /etc/letsencrypt/live/irc.yourdomain.com/privkey.pem \
  --web-addr 127.0.0.1:8080 \
  --web-static-dir freeq-app/dist \
  --db-path /var/lib/freeq/freeq.db \
  --server-name irc.yourdomain.com
```

### Nginx reverse proxy

```nginx
server {
    listen 443 ssl;
    server_name irc.yourdomain.com;

    ssl_certificate /etc/letsencrypt/live/irc.yourdomain.com/fullchain.pem;
    ssl_certificate_key /etc/letsencrypt/live/irc.yourdomain.com/privkey.pem;

    # WebSocket
    location /irc {
        proxy_pass http://127.0.0.1:8080;
        proxy_http_version 1.1;
        proxy_set_header Upgrade $http_upgrade;
        proxy_set_header Connection "upgrade";
        proxy_set_header Host $host;
        proxy_read_timeout 86400;
    }

    # API + Auth + Verifiers
    location ~ ^/(api|auth|verify|client-metadata\.json) {
        proxy_pass http://127.0.0.1:8080;
        proxy_set_header Host $host;
        proxy_set_header X-Real-IP $remote_addr;
    }

    # Upload (larger body limit)
    location /api/v1/upload {
        proxy_pass http://127.0.0.1:8080;
        proxy_set_header Host $host;
        client_max_body_size 12m;
    }

    # Static files
    location / {
        proxy_pass http://127.0.0.1:8080;
        proxy_set_header Host $host;
    }
}
```

### Systemd service

```ini
[Unit]
Description=freeq IRC server
After=network.target

[Service]
Type=simple
User=freeq
WorkingDirectory=/opt/freeq
ExecStart=/opt/freeq/target/release/freeq-server \
  --listen-addr 0.0.0.0:6667 \
  --tls-listen-addr 0.0.0.0:6697 \
  --tls-cert /etc/letsencrypt/live/irc.yourdomain.com/fullchain.pem \
  --tls-key /etc/letsencrypt/live/irc.yourdomain.com/privkey.pem \
  --web-addr 127.0.0.1:8080 \
  --web-static-dir freeq-app/dist \
  --db-path /var/lib/freeq/freeq.db \
  --server-name irc.yourdomain.com
Restart=always
RestartSec=5

[Install]
WantedBy=multi-user.target
```

```bash
sudo systemctl enable --now freeq-server
```

## GitHub credential verifier

To enable the built-in GitHub org membership verifier:

1. Create a GitHub OAuth App:
   - Settings → Developer settings → OAuth Apps → New
   - **Authorization callback URL**: `https://irc.yourdomain.com/verify/github/callback`
   - Note the Client ID and Client Secret

2. Set environment variables:
   ```bash
   export GITHUB_CLIENT_ID=your_client_id
   export GITHUB_CLIENT_SECRET=your_client_secret
   ```

3. The verifier auto-enables when these variables are present.

4. Channel ops can use it:
   ```
   /POLICY #channel SET Membership required
   /POLICY #channel REQUIRE github_membership issuer=did:web:irc.yourdomain.com:verify url=/verify/github/start label=Verify_with_GitHub
   ```

## Federation

Add `--iroh` to enable QUIC federation:

```bash
./target/release/freeq-server \
  --iroh \
  --listen-addr 0.0.0.0:6667 \
  --web-addr 127.0.0.1:8080 \
  --server-name server-a.example.com
```

Note the iroh endpoint ID from the log, then on the peer:

```bash
./target/release/freeq-server \
  --iroh \
  --listen-addr 0.0.0.0:6668 \
  --server-name server-b.example.com \
  --s2s-peers <server-a-endpoint-id>
```

## Configuration reference

| Flag | Description | Default |
|------|-------------|---------|
| `--listen-addr` | IRC listen address | `0.0.0.0:6667` |
| `--tls-listen-addr` | TLS IRC listen address | (disabled) |
| `--tls-cert` | TLS certificate path | — |
| `--tls-key` | TLS private key path | — |
| `--web-addr` | HTTP/WebSocket listen address | (disabled) |
| `--web-static-dir` | Path to web client dist/ | (disabled) |
| `--db-path` | SQLite database path | (in-memory) |
| `--server-name` | Server hostname | `localhost` |
| `--iroh` | Enable iroh QUIC federation | false |
| `--s2s-peers` | Comma-separated peer endpoint IDs | — |

| Environment Variable | Description |
|---------------------|-------------|
| `GITHUB_CLIENT_ID` | GitHub OAuth app client ID |
| `GITHUB_CLIENT_SECRET` | GitHub OAuth app client secret |
