# Deployment

## Initial Setup (Ubuntu VPS)

```sh
git clone https://github.com/chad/freeq.git
cd freeq
./deploy/setup.sh yourdomain.com [--nginx] [--iroh]
```

**Options:**
- `--nginx` — Set up nginx reverse proxy with TLS (runs certbot)
- `--iroh` — Enable iroh transport for S2S federation

The setup script:

1. Creates a dedicated `freeq` system user (no login, no home, no sudo)
2. Checks for missing apt packages and prompts to install
3. Checks for Rust/Node.js and prompts to install if missing
4. Builds the server and web app
5. Obtains a TLS cert via certbot (if `--nginx` and not already present)
6. Sets up ssl-cert group for non-root cert access
7. Generates and installs a systemd service from template
8. Creates `/etc/freeq/secrets` for environment variables
9. Creates `/var/lib/freeq/` for database storage
10. Optionally sets up nginx reverse proxy (if `--nginx`)
11. Opens firewall ports
12. Starts (or restarts) the service

The script is **idempotent** — safe to run multiple times.

## Subsequent Deploys

```sh
./deploy/deploy.sh
```

Pulls latest code, rebuilds server and web app, restarts the service.

## Secrets

Add environment variables to `/etc/freeq/secrets`. The systemd service loads this file automatically.

```sh
sudo vim /etc/freeq/secrets
```

The file is owned by `root:freeq` with mode 640 (readable by the freeq user).

## Manual Service Management

```sh
sudo systemctl status freeq-server   # Check status
sudo systemctl restart freeq-server  # Restart
sudo systemctl stop freeq-server     # Stop
sudo journalctl -u freeq-server -f   # Tail logs
```

## Files

| File | Purpose |
|------|---------|
| `setup.sh` | Initial setup (installs deps, builds, configures services) |
| `deploy.sh` | Subsequent deploys (pull, build, restart) |
| `freeq-server.service.template` | Systemd unit template (setup.sh substitutes variables) |
| `nginx.conf.template` | Nginx config template (setup.sh substitutes variables) |
| `freeq-server.service` | Chad's example systemd unit (reference only) |
| `nginx-irc-freeq-at.conf` | Chad's production nginx config (reference only) |
| `irc/` | Miren deployment (see below) |

## Alternative: Miren Deployment

[Miren](https://miren.dev/) is a self-hosted PaaS. The `irc/` subdirectory contains a complete deployment for Miren:

```sh
cd deploy/irc
./deploy.sh
```

This script:
1. Copies the workspace to a temp directory
2. Generates a Miren app config and Procfile
3. Runs `miren deploy -f`
4. Sets the route (e.g. `irc.freeq.at`)

The Procfile runs freeq-server with data stored at `/app/data/`. Miren sets `$PORT` for the web interface.

**Requirements:** Miren CLI installed and configured, route DNS pointing to your Miren instance.

## Paths

| Path | Purpose |
|------|---------|
| `/var/lib/freeq/freeq.db` | SQLite database |
| `/etc/freeq/secrets` | Environment variables (secrets) |
| `/etc/systemd/system/freeq-server.service` | Systemd unit |

## Manual Setup

If you prefer to set things up manually, the templates use these placeholders:

- `{{DOMAIN}}` — your domain (e.g. freeq.example.com)
- `{{USER}}` — system user running the service (default: `freeq`)
- `{{REPO_DIR}}` — path to the freeq repo

Example:
```sh
sed -e 's|{{DOMAIN}}|freeq.example.com|g' \
    -e 's|{{USER}}|freeq|g' \
    -e 's|{{REPO_DIR}}|/home/ubuntu/freeq|g' \
    deploy/freeq-server.service.template > /tmp/freeq-server.service

# Optional: add --iroh flag
sed -i 's|--server-name freeq.example.com \\|--server-name freeq.example.com \\\n    --iroh \\|' /tmp/freeq-server.service
```
