# Fermi DEX — Offchain Stack Production Deployment Guide

Deploy the relayer, harness, HTTP bridge, and frontend on a fresh machine pointing at Solana devnet.

---

## Architecture Overview

```
                    ┌─────────────┐
                    │   Nginx     │ :443 (HTTPS)
                    │   Gateway   │ :80  (redirect)
                    └──┬────┬──┬──┘
                       │    │  │
          /harness/*   │    │  │  /bridge/*          /*
         ┌─────────────┘    │  └──────────────┐      │
         ▼                  │                 ▼      ▼
  ┌──────────────┐   ┌─────┴──────┐   ┌──────────────────┐
  │   Harness    │   │  Frontend  │   │   HTTP Bridge     │
  │   (TS)       │   │  (static)  │   │   (TS)            │
  │   :9091      │   │  /var/lib/ │   │   :9092           │
  └──────┬───────┘   └────────────┘   └────────┬──────────┘
         │                                      │ gRPC
         │  POST /ingest/relay-intent           │
         ◄──────────────────────────────────────┤
         │                               ┌──────┴──────┐
         │                               │   Relayer   │
         │                               │   (Rust)    │
         │                               │   :9090 gRPC│
         │                               │   :9093 HTTP│
         │                               └──────┬──────┘
         │                                      │
         └──────────────────────────────────────┘
                            │
                    ┌───────▼───────┐
                    │  Solana       │
                    │  Devnet RPC   │
                    └───────────────┘
```

| Component | Port | Protocol | Purpose |
|-----------|------|----------|---------|
| Relayer (gRPC) | 9090 | gRPC | Sequencing & order submission (write path) |
| Harness | 9091 | HTTP/SSE | Market state, user state, trade history (read path) |
| HTTP Bridge | 9092 | HTTP | REST-to-gRPC translation for browser clients |
| Relayer (HTTP) | 9093 | HTTP | Health checks & Prometheus metrics |
| Nginx | 443/80 | HTTPS | Reverse proxy, static frontend, TLS termination |

---

## Prerequisites

| Requirement | Notes |
|-------------|-------|
| **OS** | Amazon Linux 2023 / Ubuntu 22.04+ / Debian 12+ |
| **Instance** | Minimum 4 vCPU, 8 GB RAM (Rust build needs ~6 GB) |
| **Disk** | 30 GB+ (Rust build artifacts are large) |
| **Network** | Outbound HTTPS to GitHub and Solana devnet RPC |
| **User** | `ec2-user` (or adjust `DEPLOY_USER` in `deploy.sh`) |
| **On-chain bootstrap** | Execution queue + group must already exist on devnet |

---

## Quick Start

```bash
# 1. SSH into your fresh machine
ssh ec2-user@<your-ip>

# 2. Clone the deployment repo (or scp the files)
git clone https://github.com/Fermi-DEX/mng-v4 ~/stagin4/mng-v4 --branch v5b
git clone https://github.com/zerooo111/fermilabs-frontend ~/stagin4/fermilabs-frontend --branch experimental

# 3. Copy deploy.sh and the scripts/ + ops/ directories into ~/stagin4/
#    (These are already in the mng-v4 repo's parent directory)

# 4. Provide the e2e config JSON (from on-chain bootstrap)
#    Place it at: ~/stagin4/mng-v4/.devnet/run/execution-queue-e2e-<GROUP_NUM>.json
#    Or set E2E_CONFIG_PATH before running deploy.sh

# 5. Provide TLS certificates (or generate self-signed for testing)
sudo mkdir -p /etc/nginx/certs
sudo openssl req -x509 -nodes -days 365 -newkey rsa:2048 \
  -keyout /etc/nginx/certs/fermi-le.key \
  -out /etc/nginx/certs/fermi-le.crt \
  -subj '/CN=your-domain.com'

# 6. Run the deployment
chmod +x ~/stagin4/deploy.sh
~/stagin4/deploy.sh
```

---

## What `deploy.sh` Does (Step-by-Step)

| Step | Action |
|------|--------|
| 1 | Installs system packages (git, curl, jq, nginx, clang, cmake, etc.) |
| 2 | Installs Rust 1.70.0 via rustup |
| 3 | Installs Node.js 18 via nvm, plus pnpm and yarn |
| 4 | Installs Solana CLI 1.16.7; generates keypair if none exists |
| 5 | Clones `mng-v4` and `fermilabs-frontend` repos (or pulls latest) |
| 6 | Builds the relayer binary (`cargo build --release`) |
| 7 | Installs mng-v4 TypeScript dependencies (`yarn install`) |
| 8 | Installs frontend dependencies (`pnpm install`) |
| 9 | Validates the e2e config JSON exists |
| 10 | Renders the `devnet-stack.env` environment file |
| 11 | Patches run scripts to use the release binary |
| 12 | Installs systemd services and starts the full stack |
| 13 | Health-checks all services |

---

## Configuration

### Environment Variables

Set these **before** running `deploy.sh` to override defaults:

```bash
# Solana RPC endpoint (use a private RPC for production reliability)
export SOLANA_URL="https://devnet.helius-rpc.com/?api-key=YOUR_KEY"

# On-chain group number (must match your bootstrap)
export GROUP_NUM=9125

# Path to the e2e config JSON if not in the default location
export E2E_CONFIG_PATH=/path/to/execution-queue-e2e-9125.json

# Keypair paths (defaults to ~/.config/solana/id.json)
export MB_PAYER_KEYPAIR=/path/to/payer.json
export CTM_RELAYER_PAYER_KEYPAIR=/path/to/relayer-payer.json
export CTM_RELAYER_CTM_KEYPAIR=/path/to/ctm-signer.json

# Airdrop settings (devnet only)
export HARNESS_ENABLE_AIRDROP=true
export HARNESS_AIRDROP_DEPOSIT_UI_AMOUNT=1000
export HARNESS_AIRDROP_AUTO_CREATE_MANGO_ACCOUNT=true

# TLS certificate paths
export NGINX_CERT_PATH=/etc/nginx/certs/fermi-le.crt
export NGINX_KEY_PATH=/etc/nginx/certs/fermi-le.key
```

### The E2E Config JSON

This file is produced by the on-chain bootstrap step and contains all derived addresses. Example structure:

```json
{
  "cluster": "devnet",
  "clusterUrl": "https://devnet.helius-rpc.com/?api-key=...",
  "programId": "7ftfLAYEtDrz8xjhaqa6wUYMrjrmbZ3tjw7J7Rb3QA37",
  "groupNum": 9125,
  "group": "<GROUP_PUBKEY>",
  "executionQueue": "<QUEUE_PUBKEY>",
  "executionQueueBuffer": "<BUFFER_PUBKEY>",
  "ctmSigner": "<CTM_SIGNER_PUBKEY>",
  "usdcMint": "<USDC_MINT>",
  "perpMarketIndex": 0,
  "maker": { "keypairPath": "...", "owner": "...", "mangoAccount": "..." },
  "taker": { "keypairPath": "...", "owner": "...", "mangoAccount": "..." }
}
```

If you haven't bootstrapped on-chain yet, run:
```bash
cd ~/stagin4/mng-v4
yarn execution-queue-local-perp-e2e-bootstrap
```

---

## Service Management

All offchain services are managed via systemd under the `stagin4-devnet.target` unit group.

### Start / Stop / Restart

```bash
# Full stack
sudo systemctl start   stagin4-devnet.target
sudo systemctl stop    stagin4-devnet.target
sudo systemctl restart stagin4-devnet.target

# Individual services
sudo systemctl restart stagin4-devnet-relayer.service
sudo systemctl restart stagin4-devnet-harness.service
sudo systemctl restart stagin4-devnet-bridge.service

# Frontend rebuild (one-shot)
sudo systemctl restart stagin4-devnet-frontend-build.service
```

### Service Dependency Order

```
stagin4-devnet-harness.service          (starts first)
  └─► stagin4-devnet-relayer.service    (waits for harness)
       └─► stagin4-devnet-bridge.service (waits for relayer + harness)

stagin4-devnet-frontend-build.service   (independent, one-shot)
stagin4-devnet-quoter.service           (independent, market maker bot)
stagin4-devnet-watchdog.timer           (every 15s health check)
nginx.service                           (reverse proxy)
```

### Logs

```bash
# Via journalctl
journalctl -u stagin4-devnet-harness -f --no-pager
journalctl -u stagin4-devnet-relayer -f --no-pager
journalctl -u stagin4-devnet-bridge  -f --no-pager

# Via log files
tail -f ~/stagin4/mng-v4/.devnet/logs/continuum-harness.log
tail -f ~/stagin4/mng-v4/.devnet/logs/ctm-relayer.log
tail -f ~/stagin4/mng-v4/.devnet/logs/ctm-relayer-http-bridge.log
```

---

## Health Checks

```bash
# Harness
curl http://127.0.0.1:9091/healthz | jq .

# Bridge
curl http://127.0.0.1:9092/healthz | jq .

# Relayer
curl http://127.0.0.1:9093/healthz | jq .

# Relayer Prometheus metrics
curl http://127.0.0.1:9093/metrics

# Full stack via nginx
curl -k https://127.0.0.1/harness/healthz | jq .
curl -k https://127.0.0.1/bridge/healthz | jq .
```

The watchdog timer (`stagin4-devnet-watchdog.timer`) runs every 15 seconds and automatically restarts any unhealthy service.

---

## Updating

```bash
# Pull latest code
cd ~/stagin4/mng-v4 && git pull origin v5b
cd ~/stagin4/fermilabs-frontend && git pull origin experimental

# Rebuild relayer (only needed if Rust code changed)
cd ~/stagin4/mng-v4 && cargo build --release -p service-mango-execution-engine

# Reinstall TS deps (only needed if package.json changed)
cd ~/stagin4/mng-v4 && yarn install
cd ~/stagin4/fermilabs-frontend && pnpm install

# Re-render env and restart
~/stagin4/scripts/render_devnet_runtime.sh
sudo systemctl restart stagin4-devnet.target
sudo systemctl restart stagin4-devnet-frontend-build.service
sudo systemctl reload nginx
```

---

## Troubleshooting

### Relayer won't start
```bash
# Check the binary exists
ls -la ~/stagin4/mng-v4/target/release/service-mango-execution-engine

# Check keypair has SOL for transaction fees
solana balance ~/.config/solana/id.json --url devnet

# Check env file was rendered
cat ~/stagin4/mng-v4/.devnet/systemd/devnet-stack.env
```

### Harness shows `airdrop_enabled: false`
The harness needs the payer keypair to have SOL and a USDC token account. For devnet:
```bash
solana airdrop 2 --url devnet
```

### Frontend shows blank page
```bash
# Check the build succeeded
ls /var/lib/stagin4/frontend/index.html

# Rebuild
sudo systemctl restart stagin4-devnet-frontend-build.service

# Check nginx is serving it
sudo nginx -t && sudo systemctl reload nginx
```

### Services crash-loop
```bash
# Check recent logs for the failing service
journalctl -u stagin4-devnet-relayer --since "5 min ago" --no-pager

# Verify the e2e config is valid
cat ~/stagin4/mng-v4/.devnet/run/execution-queue-e2e-*.json | jq .

# Verify on-chain accounts exist
solana account <GROUP_PK> --url devnet
solana account <EXECUTION_QUEUE_PK> --url devnet
```

### Port conflicts
```bash
# Check what's listening
ss -tlnp | grep -E '909[0-3]|8070|443|80'
```

---

## File Layout

```
~/stagin4/
├── deploy.sh                              ← This deployment script
├── setup.md                               ← This guide
├── scripts/
│   ├── render_devnet_runtime.sh           ← Generates devnet-stack.env
│   ├── run_devnet_relayer.sh              ← Relayer launch wrapper
│   ├── run_devnet_harness.sh              ← Harness launch wrapper
│   ├── run_devnet_bridge.sh               ← Bridge launch wrapper
│   ├── build_devnet_frontend.sh           ← Frontend build script
│   ├── run_devnet_quoter.sh               ← Quoter bot launcher
│   ├── install_devnet_systemd.sh          ← Systemd installer
│   └── devnet_stack_watchdog.sh           ← Health monitor
├── ops/systemd/
│   ├── stagin4-devnet.target              ← Service group
│   ├── stagin4-devnet-harness.service
│   ├── stagin4-devnet-relayer.service
│   ├── stagin4-devnet-bridge.service
│   ├── stagin4-devnet-frontend-build.service
│   ├── stagin4-devnet-quoter.service
│   ├── stagin4-devnet-watchdog.service
│   └── stagin4-devnet-watchdog.timer
├── mng-v4/                                ← Core program + offchain services
│   ├── target/release/
│   │   └── service-mango-execution-engine ← Relayer binary
│   ├── ts/client/scripts/execution-queue/
│   │   ├── continuum-state-harness.ts     ← Harness entry point
│   │   └── ctm-relayer-http-bridge.ts     ← Bridge entry point
│   ├── .devnet/
│   │   ├── run/                           ← Runtime state (e2e config, sequences)
│   │   ├── logs/                          ← Service logs
│   │   └── systemd/devnet-stack.env       ← Generated environment file
│   └── keypairs/                          ← Persistent keypairs
└── fermilabs-frontend/                    ← Trading UI (React/Vite)
    └── dist/                              ← Build output → copied to /var/lib/stagin4/frontend/
```

---

## Security Notes

- The Solana keypair at `~/.config/solana/id.json` is the **payer** for all transactions. Fund it with devnet SOL but keep it secured.
- The relayer gRPC port (9090) is bound to `127.0.0.1` — not exposed externally. External access goes through nginx → bridge (9092).
- All internal service ports (9090-9093) bind to localhost only.
- Nginx terminates TLS. Use real certificates (e.g., Let's Encrypt) for production.
- The watchdog auto-restarts crashed services but will not fix configuration errors — check logs if a service keeps restarting.
