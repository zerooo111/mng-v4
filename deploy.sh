#!/usr/bin/env bash
###############################################################################
# deploy.sh — Fresh-machine production deployment of Fermi DEX offchain stack
#              (relayer + harness + bridge + frontend) pointing at Solana devnet
#
# Usage:
#   chmod +x deploy.sh && ./deploy.sh
#
# Prerequisites: Amazon Linux 2023 (or similar) EC2 instance with internet.
# The script is idempotent — safe to re-run.
###############################################################################
set -euo pipefail

###############################################################################
# 0. Configuration — edit these before first run
###############################################################################
DEPLOY_USER="ec2-user"
DEPLOY_HOME="/home/${DEPLOY_USER}"
INSTALL_DIR="${DEPLOY_HOME}/stagin4"

# Git repos
MNG_REPO="https://github.com/Fermi-DEX/mng-v4"
MNG_BRANCH="v5b"
FRONTEND_REPO="https://github.com/zerooo111/fermilabs-frontend"
FRONTEND_BRANCH="experimental"

# Toolchain versions
RUST_VERSION="1.70.0"
NODE_VERSION="18"
SOLANA_VERSION="1.16.7"

# Solana RPC (override with your own RPC for production reliability)
SOLANA_URL="${SOLANA_URL:-https://api.devnet.solana.com}"

# Group number — must match your on-chain bootstrap
GROUP_NUM="${GROUP_NUM:-9125}"

# Path to the e2e config JSON produced by the on-chain bootstrap step.
# If you already have one, set this env var before running the script.
E2E_CONFIG_PATH="${E2E_CONFIG_PATH:-}"

# TLS certificate paths for nginx (must exist before nginx starts)
NGINX_CERT_PATH="${NGINX_CERT_PATH:-/etc/nginx/certs/fermi-le.crt}"
NGINX_KEY_PATH="${NGINX_KEY_PATH:-/etc/nginx/certs/fermi-le.key}"

###############################################################################
# Helpers
###############################################################################
log()  { echo -e "\n\033[1;34m>>> $*\033[0m"; }
warn() { echo -e "\033[1;33mWARN: $*\033[0m"; }
die()  { echo -e "\033[1;31mFATAL: $*\033[0m" >&2; exit 1; }

command_exists() { command -v "$1" &>/dev/null; }

###############################################################################
# 1. System packages
###############################################################################
log "Installing system dependencies"
sudo dnf groupinstall -y "Development Tools" 2>/dev/null || sudo yum groupinstall -y "Development Tools" 2>/dev/null || true
sudo dnf install -y \
  git curl wget jq openssl-devel pkg-config clang cmake perl \
  nginx protobuf-compiler \
  2>/dev/null || \
sudo yum install -y \
  git curl wget jq openssl-devel pkgconfig clang cmake perl \
  nginx protobuf-compiler \
  2>/dev/null || true

###############################################################################
# 2. Rust toolchain
###############################################################################
if ! command_exists rustc || [[ "$(rustc --version)" != *"${RUST_VERSION}"* ]]; then
  log "Installing Rust ${RUST_VERSION}"
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain "${RUST_VERSION}"
  source "${DEPLOY_HOME}/.cargo/env"
else
  log "Rust ${RUST_VERSION} already installed"
  source "${DEPLOY_HOME}/.cargo/env" 2>/dev/null || true
fi

###############################################################################
# 3. Node.js (via nvm)
###############################################################################
export NVM_DIR="${DEPLOY_HOME}/.nvm"
if [[ ! -d "${NVM_DIR}" ]]; then
  log "Installing nvm + Node.js ${NODE_VERSION}"
  curl -o- https://raw.githubusercontent.com/nvm-sh/nvm/v0.39.7/install.sh | bash
fi
# shellcheck disable=SC1091
[ -s "${NVM_DIR}/nvm.sh" ] && source "${NVM_DIR}/nvm.sh"
nvm install "${NODE_VERSION}" 2>/dev/null || true
nvm use "${NODE_VERSION}"

# pnpm (for frontend)
if ! command_exists pnpm; then
  log "Installing pnpm"
  npm install -g pnpm
fi

# yarn (for mng-v4 TypeScript)
if ! command_exists yarn; then
  log "Installing yarn"
  corepack enable 2>/dev/null || npm install -g yarn
fi

###############################################################################
# 4. Solana CLI
###############################################################################
SOLANA_BIN_DIR="${DEPLOY_HOME}/.local/solana-${SOLANA_VERSION}-release/bin"
if [[ ! -x "${SOLANA_BIN_DIR}/solana" ]]; then
  log "Installing Solana CLI ${SOLANA_VERSION}"
  sh -c "$(curl -sSfL https://release.solana.com/v${SOLANA_VERSION}/install)" -- "${SOLANA_VERSION}"
fi
export PATH="${SOLANA_BIN_DIR}:${PATH}"

# Generate a keypair if none exists
if [[ ! -f "${DEPLOY_HOME}/.config/solana/id.json" ]]; then
  log "Generating Solana keypair"
  mkdir -p "${DEPLOY_HOME}/.config/solana"
  solana-keygen new --no-bip39-passphrase -o "${DEPLOY_HOME}/.config/solana/id.json"
fi
solana config set --url "${SOLANA_URL}" 2>/dev/null || true

###############################################################################
# 5. Clone repositories
###############################################################################
mkdir -p "${INSTALL_DIR}"

# --- mng-v4 (relayer, harness, bridge, on-chain program) ---
MNG_DIR="${INSTALL_DIR}/mng-v4"
if [[ ! -d "${MNG_DIR}/.git" ]]; then
  log "Cloning mng-v4 (branch: ${MNG_BRANCH})"
  git clone --branch "${MNG_BRANCH}" --single-branch "${MNG_REPO}" "${MNG_DIR}"
else
  log "Updating mng-v4"
  cd "${MNG_DIR}" && git fetch origin && git checkout "${MNG_BRANCH}" && git pull origin "${MNG_BRANCH}"
fi

# --- fermilabs-frontend ---
FRONTEND_DIR="${INSTALL_DIR}/fermilabs-frontend"
if [[ ! -d "${FRONTEND_DIR}/.git" ]]; then
  log "Cloning fermilabs-frontend (branch: ${FRONTEND_BRANCH})"
  git clone --branch "${FRONTEND_BRANCH}" --single-branch "${FRONTEND_REPO}" "${FRONTEND_DIR}"
else
  log "Updating fermilabs-frontend"
  cd "${FRONTEND_DIR}" && git fetch origin && git checkout "${FRONTEND_BRANCH}" && git pull origin "${FRONTEND_BRANCH}"
fi

###############################################################################
# 6. Build relayer (Rust)
###############################################################################
log "Building relayer (service-mango-execution-engine) — this may take 10-20 min on first build"
cd "${MNG_DIR}"
cargo build --release -p service-mango-execution-engine 2>&1 | tail -5

# Use release binary
EXECUTION_ENGINE_BIN="${MNG_DIR}/target/release/service-mango-execution-engine"
if [[ ! -x "${EXECUTION_ENGINE_BIN}" ]]; then
  die "Relayer binary not found at ${EXECUTION_ENGINE_BIN}"
fi

###############################################################################
# 7. Install TypeScript dependencies (harness + bridge)
###############################################################################
log "Installing mng-v4 TypeScript dependencies"
cd "${MNG_DIR}"
yarn install 2>&1 | tail -5

###############################################################################
# 8. Install frontend dependencies
###############################################################################
log "Installing frontend dependencies"
cd "${FRONTEND_DIR}"
pnpm install 2>&1 | tail -5

###############################################################################
# 9. Prepare runtime directories & e2e config
###############################################################################
RUN_DIR="${MNG_DIR}/.devnet/run"
LOG_DIR="${MNG_DIR}/.devnet/logs"
SYSTEMD_DIR="${MNG_DIR}/.devnet/systemd"
mkdir -p "${RUN_DIR}" "${LOG_DIR}" "${SYSTEMD_DIR}" "${FRONTEND_DIR}/.localdev"

if [[ -n "${E2E_CONFIG_PATH}" && -f "${E2E_CONFIG_PATH}" ]]; then
  # Copy the provided config into the run dir
  CFG_DEST="${RUN_DIR}/execution-queue-e2e-${GROUP_NUM}.json"
  if [[ "${E2E_CONFIG_PATH}" != "${CFG_DEST}" ]]; then
    cp "${E2E_CONFIG_PATH}" "${CFG_DEST}"
  fi
else
  # Try to find an existing one
  E2E_CONFIG_PATH="$(find "${RUN_DIR}" -maxdepth 1 -name 'execution-queue-e2e-*.json' | sort | tail -n 1 || true)"
  if [[ -z "${E2E_CONFIG_PATH}" || ! -f "${E2E_CONFIG_PATH}" ]]; then
    die "No e2e config JSON found. You must run on-chain bootstrap first, or supply E2E_CONFIG_PATH.
See setup.md for details."
  fi
fi

###############################################################################
# 10. Render environment file
###############################################################################
log "Rendering devnet environment file"
cd "${INSTALL_DIR}"
chmod +x scripts/render_devnet_runtime.sh 2>/dev/null || true

# Source node for render script
export GROUP_NUM SOLANA_URL E2E_CONFIG_PATH
export MB_PAYER_KEYPAIR="${MB_PAYER_KEYPAIR:-${DEPLOY_HOME}/.config/solana/id.json}"
export CTM_RELAYER_PAYER_KEYPAIR="${CTM_RELAYER_PAYER_KEYPAIR:-${MB_PAYER_KEYPAIR}}"
export CTM_RELAYER_CTM_KEYPAIR="${CTM_RELAYER_CTM_KEYPAIR:-${MB_PAYER_KEYPAIR}}"
export EXECUTION_QUEUE_ADMIN_KEYPAIR="${EXECUTION_QUEUE_ADMIN_KEYPAIR:-${MB_PAYER_KEYPAIR}}"
export HARNESS_ENABLE_AIRDROP="${HARNESS_ENABLE_AIRDROP:-true}"
export HARNESS_AIRDROP_DEPOSIT_UI_AMOUNT="${HARNESS_AIRDROP_DEPOSIT_UI_AMOUNT:-1000}"
export HARNESS_AIRDROP_AUTO_CREATE_MANGO_ACCOUNT="${HARNESS_AIRDROP_AUTO_CREATE_MANGO_ACCOUNT:-true}"
export QUOTER_BOT_COUNT="${QUOTER_BOT_COUNT:-2}"
export QUOTER_INTERVAL_MS="${QUOTER_INTERVAL_MS:-2000}"
export QUOTER_PRICE_RANGE_BPS="${QUOTER_PRICE_RANGE_BPS:-200}"

ENV_PATH="$(bash scripts/render_devnet_runtime.sh)"
log "Environment written to: ${ENV_PATH}"

###############################################################################
# 11. Patch run scripts to use release binary
###############################################################################
RELAYER_SCRIPT="${INSTALL_DIR}/scripts/run_devnet_relayer.sh"
if grep -q 'target/debug/' "${RELAYER_SCRIPT}" 2>/dev/null; then
  log "Patching relayer script to use release binary"
  sed -i 's|target/debug/service-mango-execution-engine|target/release/service-mango-execution-engine|g' "${RELAYER_SCRIPT}"
fi

###############################################################################
# 12. Make all scripts executable
###############################################################################
chmod +x "${INSTALL_DIR}/scripts/"*.sh

###############################################################################
# 13. TLS certificates check
###############################################################################
if [[ ! -f "${NGINX_CERT_PATH}" || ! -f "${NGINX_KEY_PATH}" ]]; then
  warn "TLS certificates not found at ${NGINX_CERT_PATH} / ${NGINX_KEY_PATH}"
  warn "Nginx HTTPS will fail until you provide certificates."
  warn "For testing, generate self-signed certs:"
  warn "  sudo mkdir -p /etc/nginx/certs"
  warn "  sudo openssl req -x509 -nodes -days 365 -newkey rsa:2048 \\"
  warn "    -keyout ${NGINX_KEY_PATH} -out ${NGINX_CERT_PATH} \\"
  warn "    -subj '/CN=localhost'"
fi

###############################################################################
# 14. Install & start systemd services
###############################################################################
log "Installing systemd services"
sudo bash "${INSTALL_DIR}/scripts/install_devnet_systemd.sh"

###############################################################################
# 15. Verify
###############################################################################
log "Waiting for services to start (15s)..."
sleep 15

echo ""
echo "=============================================="
echo "  Service Status"
echo "=============================================="

check_health() {
  local name="$1" url="$2"
  if curl -fsS "${url}" >/dev/null 2>&1; then
    echo "  ✓ ${name} — healthy"
  else
    echo "  ✗ ${name} — not responding (check logs)"
  fi
}

check_health "Harness  (9091)" "http://127.0.0.1:9091/healthz"
check_health "Bridge   (9092)" "http://127.0.0.1:9092/healthz"
check_health "Relayer  (9093)" "http://127.0.0.1:9093/healthz"
check_health "Nginx    (443)"  "https://127.0.0.1/ -k"

echo ""
echo "Logs:"
echo "  journalctl -u stagin4-devnet-harness -f"
echo "  journalctl -u stagin4-devnet-relayer -f"
echo "  journalctl -u stagin4-devnet-bridge  -f"
echo "  tail -f ${LOG_DIR}/*.log"
echo ""
echo "Manage:"
echo "  sudo systemctl status  stagin4-devnet.target"
echo "  sudo systemctl restart stagin4-devnet.target"
echo "  sudo systemctl stop    stagin4-devnet.target"
echo ""
log "Deployment complete."
