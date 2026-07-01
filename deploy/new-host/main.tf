# Self-contained freeq host on Hetzner Cloud, managed by miren.
#
# Provisions ONE Ubuntu 24.04 box sized from the measured freeq footprint
# (see README.md): miren control plane (~1 GB, ~1 core) + freeq-server
# (~70 MB) + freeq-site docs + headroom for AV and in-cluster Rust builds.
#
# Usage:
#   export HCLOUD_TOKEN=...            # Hetzner Cloud API token (new project)
#   terraform init && terraform apply
#   # then follow README.md: register the cluster to your new miren org and
#   # `miren deploy` freeq-server + freeq-site.
#
# DigitalOcean instead? See README.md — swap this provider/resource block for
# the `digitalocean_droplet` equivalent; cloud-init.yaml is provider-agnostic.

terraform {
  required_providers {
    hcloud = {
      source  = "hetznercloud/hcloud"
      version = "~> 1.48"
    }
  }
}

variable "hcloud_token" {
  type        = string
  sensitive   = true
  description = "Hetzner Cloud API token (env: HCLOUD_TOKEN)"
  default     = null
}

variable "server_type" {
  type        = string
  description = "CCX23 = 4 dedicated vCPU / 16 GB / 160 GB (recommended). CAX21 = 4 ARM vCPU / 8 GB (cost-first)."
  default     = "ccx23"
}

variable "location" {
  type        = string
  description = "hel1/fsn1/nbg1 (EU) or ash/hil (US). Pick nearest your users."
  default     = "ash"
}

variable "admin_cidrs" {
  type        = list(string)
  description = "IPs allowed to reach SSH (22) and the miren API (8443). Lock this down — do NOT leave 0.0.0.0/0."
  default     = ["0.0.0.0/0"]
}

variable "ssh_public_key" {
  type        = string
  description = "Your SSH public key (contents, e.g. file(\"~/.ssh/id_ed25519.pub\"))."
}

variable "expose_native_irc" {
  type        = bool
  description = "Open 6697 (IRC-over-TLS) for native IRC clients. Web-app-only companies can leave this false."
  default     = true
}

provider "hcloud" {
  token = coalesce(var.hcloud_token, "")
}

resource "hcloud_ssh_key" "admin" {
  name       = "freeq-admin"
  public_key = var.ssh_public_key
}

resource "hcloud_firewall" "freeq" {
  name = "freeq"

  # SSH + miren control-plane API — restricted to admin IPs only.
  rule {
    direction  = "in"
    protocol   = "tcp"
    port       = "22"
    source_ips = var.admin_cidrs
  }
  rule {
    direction  = "in"
    protocol   = "tcp"
    port       = "8443"
    source_ips = var.admin_cidrs
  }

  # Public HTTP/HTTPS — web client (WSS /irc), REST, docs, AV-over-WebSocket.
  rule {
    direction  = "in"
    protocol   = "tcp"
    port       = "80"
    source_ips = ["0.0.0.0/0", "::/0"]
  }
  rule {
    direction  = "in"
    protocol   = "tcp"
    port       = "443"
    source_ips = ["0.0.0.0/0", "::/0"]
  }

  # Native IRC-over-TLS (optional).
  dynamic "rule" {
    for_each = var.expose_native_irc ? [1] : []
    content {
      direction  = "in"
      protocol   = "tcp"
      port       = "6697"
      source_ips = ["0.0.0.0/0", "::/0"]
    }
  }

  # AV / iroh QUIC (UDP). Range keeps it simple for NAT-traversed media + S2S.
  rule {
    direction  = "in"
    protocol   = "udp"
    port       = "1024-65535"
    source_ips = ["0.0.0.0/0", "::/0"]
  }
}

resource "hcloud_server" "freeq" {
  name         = "freeq-prod"
  server_type  = var.server_type
  image        = "ubuntu-24.04"
  location     = var.location
  ssh_keys     = [hcloud_ssh_key.admin.id]
  firewall_ids = [hcloud_firewall.freeq.id]
  user_data    = file("${path.module}/cloud-init.yaml")

  public_net {
    ipv4_enabled = true
    ipv6_enabled = true
  }
}

output "ipv4" {
  value       = hcloud_server.freeq.ipv4_address
  description = "Point freeq.at / irc.freeq.at A records here once verified."
}

output "next_steps" {
  value = <<-EOT
    Host is up at ${hcloud_server.freeq.ipv4_address}.
    Now follow README.md §3-5:
      1. ssh root@${hcloud_server.freeq.ipv4_address}   # cloud-init installed the miren server
      2. Register the cluster to your NEW miren org (interactive, needs your org auth)
      3. From your laptop: miren cluster add + miren deploy freeq-server & freeq-site
      4. Cut DNS: freeq.at, www.freeq.at, irc.freeq.at -> ${hcloud_server.freeq.ipv4_address}
  EOT
}
