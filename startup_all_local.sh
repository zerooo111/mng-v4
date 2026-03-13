#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
MNG_DIR="${ROOT_DIR}/mng-v4"
FRONTEND_DIR="${ROOT_DIR}/fermilabs-frontend"

LOCALNET_DIR="${MNG_DIR}/.localnet"
LEDGER_DIR="${LOCALNET_DIR}/ledger"
LOG_DIR="${LOCALNET_DIR}/logs"
RUN_DIR="${LOCALNET_DIR}/run"
PID_DIR="${RUN_DIR}/pids"
FRONTEND_RUN_DIR="${FRONTEND_DIR}/.localdev"
EXECUTION_ENGINE_BIN="${MNG_DIR}/target/debug/service-mango-execution-engine"

SOLANA_URL="${SOLANA_URL:-http://127.0.0.1:8899}"
FRONTEND_HOST="${FRONTEND_HOST:-127.0.0.1}"
FRONTEND_PORT="${FRONTEND_PORT:-5173}"
GROUP_NUM="${GROUP_NUM:-9120}"
PERP_MARKET_INDEX="${PERP_MARKET_INDEX:-0}"
MB_PAYER_KEYPAIR="${MB_PAYER_KEYPAIR:-/home/ec2-user/.config/solana/id.json}"
CTM_RELAYER_PAYER_KEYPAIR="${CTM_RELAYER_PAYER_KEYPAIR:-${MB_PAYER_KEYPAIR}}"
CTM_RELAYER_CTM_KEYPAIR="${CTM_RELAYER_CTM_KEYPAIR:-${MB_PAYER_KEYPAIR}}"
CTM_RELAYER_IMPL="${CTM_RELAYER_IMPL:-ts}"
CTM_EXECUTION_ENGINE_HTTP_BIND_ADDR="${CTM_EXECUTION_ENGINE_HTTP_BIND_ADDR:-127.0.0.1:9093}"
HARNESS_ENABLE_AIRDROP="${HARNESS_ENABLE_AIRDROP:-true}"
HARNESS_AIRDROP_DEPOSIT_UI_AMOUNT="${HARNESS_AIRDROP_DEPOSIT_UI_AMOUNT:-1000}"

REQUIRED_SOLANA_VERSION="${REQUIRED_SOLANA_VERSION:-1.16.7}"
SOLANA_116_BIN_DIR="${SOLANA_116_BIN_DIR:-/home/ec2-user/.local/solana-1.16.7-release/bin}"
if [[ -d "${SOLANA_116_BIN_DIR}" ]]; then
  export PATH="${SOLANA_116_BIN_DIR}:${PATH}"
fi

PROGRAM_KEYPAIR="${MNG_DIR}/target/deploy/mango_v4-keypair.json"
PROGRAM_SO="${MNG_DIR}/target/deploy/mango_v4.so"
PROGRAM_ID="${PROGRAM_ID:-9nNhSkcxYFujiydpuuhVttUYBqYJQmxCzjrBofBvmutF}"
PROGRAM_ID_EFFECTIVE="${PROGRAM_ID}"

DEPLOY_MODE="${DEPLOY_MODE:-auto}" # auto|cli|preload
DEPLOY_TIMEOUT_SECS="${DEPLOY_TIMEOUT_SECS:-180}"
DEPLOY_RETRIES="${DEPLOY_RETRIES:-1}"
DEPLOY_TRANSPORT="${DEPLOY_TRANSPORT:-udp}" # udp|quic|auto
HARNESS_START_TIMEOUT_SECS="${HARNESS_START_TIMEOUT_SECS:-300}"
RELAYER_START_TIMEOUT_SECS="${RELAYER_START_TIMEOUT_SECS:-300}"
BRIDGE_START_TIMEOUT_SECS="${BRIDGE_START_TIMEOUT_SECS:-300}"
FRONTEND_START_TIMEOUT_SECS="${FRONTEND_START_TIMEOUT_SECS:-300}"
NGINX_START_TIMEOUT_SECS="${NGINX_START_TIMEOUT_SECS:-60}"
TS_NODE_TRANSPILE_ONLY="${TS_NODE_TRANSPILE_ONLY:-1}"

E2E_CONFIG_PATH="${E2E_CONFIG_PATH:-${RUN_DIR}/execution-queue-e2e-${GROUP_NUM}.json}"
BUFFER_LAYOUT_PATH="${RUN_DIR}/execution-queue-buffer-${GROUP_NUM}.json"
MAKER_KEYPAIR_PATH="${RUN_DIR}/execution-queue-maker-${GROUP_NUM}.json"
TAKER_KEYPAIR_PATH="${RUN_DIR}/execution-queue-taker-${GROUP_NUM}.json"
LANE_CONFIG_PATH="${RUN_DIR}/execution-queue-lanes-${GROUP_NUM}.json"
RELAYER_SEQUENCE_STATE_PATH="${RUN_DIR}/ctm-sequences-${GROUP_NUM}.json"
CONTINUUM_EVENT_LOG_PATH="${RUN_DIR}/continuum-harness-${GROUP_NUM}.jsonl"

VALIDATOR_PID_FILE="${PID_DIR}/validator.pid"
RELAYER_PID_FILE="${PID_DIR}/relayer.pid"
HARNESS_PID_FILE="${PID_DIR}/harness.pid"
BRIDGE_PID_FILE="${PID_DIR}/bridge.pid"
FRONTEND_PID_FILE="${FRONTEND_RUN_DIR}/frontend.pid"

mkdir -p "${LOG_DIR}" "${RUN_DIR}" "${PID_DIR}" "${FRONTEND_RUN_DIR}"

log() {
  echo "[startup] $*"
}

require_cmd() {
  local cmd="$1"
  command -v "${cmd}" >/dev/null 2>&1 || {
    echo "Missing required command: ${cmd}" >&2
    exit 1
  }
}

extract_version() {
  local value="$1"
  awk '{print $2}' <<<"${value}"
}

preflight_binaries() {
  require_cmd solana
  require_cmd solana-test-validator
  require_cmd timeout
  require_cmd curl
  require_cmd node
  require_cmd ss
  require_cmd rg

  local solana_bin validator_bin
  local solana_version validator_version
  solana_bin="$(command -v solana)"
  validator_bin="$(command -v solana-test-validator)"
  solana_version="$(extract_version "$(solana --version)")"
  validator_version="$(extract_version "$(solana-test-validator --version)")"

  log "solana binary: ${solana_bin} (${solana_version})"
  log "validator binary: ${validator_bin} (${validator_version})"

  if [[ "${solana_version}" != "${REQUIRED_SOLANA_VERSION}"* ]]; then
    echo "solana-cli version mismatch: expected ${REQUIRED_SOLANA_VERSION}.*, got ${solana_version}" >&2
    exit 1
  fi
  if [[ "${validator_version}" != "${REQUIRED_SOLANA_VERSION}"* ]]; then
    echo "solana-test-validator version mismatch: expected ${REQUIRED_SOLANA_VERSION}.*, got ${validator_version}" >&2
    exit 1
  fi
}

preflight_program_identity() {
  if [[ ! -f "${PROGRAM_KEYPAIR}" || ! -f "${PROGRAM_SO}" ]]; then
    echo "Missing deploy artifacts (${PROGRAM_SO} / ${PROGRAM_KEYPAIR})" >&2
    exit 1
  fi
  local keypair_program_id
  keypair_program_id="$(solana address -k "${PROGRAM_KEYPAIR}")"
  if [[ "${PROGRAM_ID}" != "${keypair_program_id}" ]]; then
    log "PROGRAM_ID (${PROGRAM_ID}) differs from keypair pubkey (${keypair_program_id}); using keypair pubkey."
  fi
  PROGRAM_ID_EFFECTIVE="${keypair_program_id}"
}

pid_is_running() {
  local pid_file="$1"
  if [[ ! -f "${pid_file}" ]]; then
    return 1
  fi
  local pid
  pid="$(cat "${pid_file}")"
  [[ -n "${pid}" ]] && kill -0 "${pid}" 2>/dev/null
}

stop_pid_file() {
  local pid_file="$1"
  local name="$2"
  if pid_is_running "${pid_file}"; then
    local pid
    pid="$(cat "${pid_file}")"
    log "Stopping ${name} (pid ${pid})"
    kill "${pid}" 2>/dev/null || true
    for _ in $(seq 1 50); do
      if ! kill -0 "${pid}" 2>/dev/null; then
        break
      fi
      sleep 0.1
    done
    kill -9 "${pid}" 2>/dev/null || true
  fi
  rm -f "${pid_file}"
}

stop_all() {
  stop_pid_file "${FRONTEND_PID_FILE}" "frontend"
  stop_pid_file "${BRIDGE_PID_FILE}" "bridge"
  stop_pid_file "${RELAYER_PID_FILE}" "relayer"
  stop_pid_file "${HARNESS_PID_FILE}" "harness"
  stop_pid_file "${VALIDATOR_PID_FILE}" "validator"

  pkill -f 'ts/client/scripts/execution-queue/ctm-relayer-http-bridge.ts' 2>/dev/null || true
  pkill -f 'ts/client/scripts/execution-queue/continuum-state-harness.ts' 2>/dev/null || true
  pkill -f 'ts/client/scripts/execution-queue/ctm-sequencer-relayer.ts' 2>/dev/null || true
  pkill -f 'ts/client/scripts/execution-queue/execution-queue-cranker.ts' 2>/dev/null || true
  pkill -f 'service-mango-execution-engine' 2>/dev/null || true
  pkill -f 'solana -u .* program deploy .*mango_v4.so' 2>/dev/null || true
  pkill -f 'solana-test-validator --ledger /home/ec2-user/stagin4/mng-v4/.localnet/ledger' 2>/dev/null || true
  pkill -f "vite --host ${FRONTEND_HOST} --port ${FRONTEND_PORT}" 2>/dev/null || true
}

fresh_cleanup() {
  rm -rf "${LOCALNET_DIR}"
  mkdir -p "${LEDGER_DIR}" "${LOG_DIR}" "${RUN_DIR}" "${PID_DIR}"
}

wait_for_rpc() {
  local max_tries="${1:-90}"
  for _ in $(seq 1 "${max_tries}"); do
    if curl -s "${SOLANA_URL}" \
      -H 'Content-Type: application/json' \
      -d '{"jsonrpc":"2.0","id":1,"method":"getHealth"}' | rg -q '"result":"ok"'; then
      return 0
    fi
    sleep 1
  done
  echo "RPC did not become healthy at ${SOLANA_URL}" >&2
  return 1
}

wait_for_http_ok() {
  local url="$1"
  local max_tries="${2:-120}"
  for _ in $(seq 1 "${max_tries}"); do
    if curl -fsS "${url}" >/dev/null 2>&1; then
      return 0
    fi
    sleep 1
  done
  echo "HTTP endpoint did not become ready: ${url}" >&2
  return 1
}

wait_for_port() {
  local port="$1"
  local max_tries="${2:-120}"
  for _ in $(seq 1 "${max_tries}"); do
    if ss -ltn | rg -q ":${port}[[:space:]]"; then
      return 0
    fi
    sleep 1
  done
  echo "Port ${port} did not start listening" >&2
  return 1
}

start_failure_diagnostics() {
  local service="$1"
  local pid_file="$2"
  local log_file="$3"
  echo "${service} failed to become ready." >&2
  if [[ -f "${pid_file}" ]]; then
    local pid
    pid="$(cat "${pid_file}")"
    if [[ -n "${pid}" ]] && kill -0 "${pid}" 2>/dev/null; then
      echo "${service} process is still running (pid ${pid})" >&2
    else
      echo "${service} process is not running" >&2
    fi
  else
    echo "${service} pid file missing (${pid_file})" >&2
  fi
  if [[ -f "${log_file}" ]]; then
    echo "--- ${log_file} (tail) ---" >&2
    tail -n 120 "${log_file}" >&2 || true
  else
    echo "No log file found for ${service}: ${log_file}" >&2
  fi
}

wait_for_http_or_fail() {
  local service="$1"
  local url="$2"
  local max_tries="$3"
  local pid_file="$4"
  local log_file="$5"
  if ! wait_for_http_ok "${url}" "${max_tries}"; then
    start_failure_diagnostics "${service}" "${pid_file}" "${log_file}"
    return 1
  fi
}

wait_for_port_or_fail() {
  local service="$1"
  local port="$2"
  local max_tries="$3"
  local pid_file="$4"
  local log_file="$5"
  if ! wait_for_port "${port}" "${max_tries}"; then
    start_failure_diagnostics "${service}" "${pid_file}" "${log_file}"
    return 1
  fi
}

ensure_nginx_https_gateway() {
  require_cmd nginx
  if command -v systemctl >/dev/null 2>&1; then
    if ! systemctl is-active --quiet nginx; then
      sudo systemctl start nginx
    fi
  else
    sudo nginx >/dev/null 2>&1 || nginx >/dev/null 2>&1 || true
  fi
  wait_for_port_or_fail \
    "nginx" \
    443 \
    "${NGINX_START_TIMEOUT_SECS}" \
    "/dev/null" \
    "/var/log/nginx/error.log"
}

program_is_deployed() {
  solana -u "${SOLANA_URL}" program show "${PROGRAM_ID_EFFECTIVE}" >/dev/null 2>&1
}

start_validator() {
  local mode="${1:-regular}" # regular|preload
  local args=(
    --ledger "${LEDGER_DIR}"
    --reset
  )
  if [[ "${mode}" == "preload" ]]; then
    args+=(--bpf-program "${PROGRAM_ID_EFFECTIVE}" "${PROGRAM_SO}")
  fi

  setsid solana-test-validator "${args[@]}" >"${LOG_DIR}/validator.log" 2>&1 < /dev/null &
  echo $! >"${VALIDATOR_PID_FILE}"
  wait_for_rpc
}

deploy_transport_flags() {
  case "${DEPLOY_TRANSPORT}" in
    udp)
      echo "--use-udp"
      ;;
    quic)
      echo "--use-quic"
      ;;
    auto)
      ;;
    *)
      echo "Unknown DEPLOY_TRANSPORT=${DEPLOY_TRANSPORT}" >&2
      exit 1
      ;;
  esac
}

deploy_via_cli() {
  local attempt=1
  local max_attempts=$((DEPLOY_RETRIES + 1))
  local transport_flag
  transport_flag="$(deploy_transport_flags)"

  while (( attempt <= max_attempts )); do
    log "Deploy attempt ${attempt}/${max_attempts} (mode=cli)"
    {
      echo "=== deploy attempt ${attempt} @ $(date -u +"%Y-%m-%dT%H:%M:%SZ") ==="
      timeout "${DEPLOY_TIMEOUT_SECS}" solana -u "${SOLANA_URL}" program deploy \
        ${transport_flag} \
        --program-id "${PROGRAM_KEYPAIR}" \
        "${PROGRAM_SO}"
    } >>"${LOG_DIR}/deploy.log" 2>&1 || true

    if program_is_deployed; then
      log "Program deployed: ${PROGRAM_ID_EFFECTIVE}"
      return 0
    fi
    attempt=$((attempt + 1))
  done

  return 1
}

deploy_or_preload_program() {
  case "${DEPLOY_MODE}" in
    preload)
      log "DEPLOY_MODE=preload, starting validator with --bpf-program"
      stop_pid_file "${VALIDATOR_PID_FILE}" "validator"
      start_validator preload
      program_is_deployed || {
        echo "Program verification failed in preload mode (${PROGRAM_ID_EFFECTIVE})" >&2
        return 1
      }
      ;;
    cli)
      if ! deploy_via_cli; then
        echo "Deploy failed in DEPLOY_MODE=cli; see ${LOG_DIR}/deploy.log" >&2
        return 1
      fi
      ;;
    auto)
      if ! deploy_via_cli; then
        log "CLI deploy failed/timed out; falling back to preload validator mode."
        stop_pid_file "${VALIDATOR_PID_FILE}" "validator"
        start_validator preload
        program_is_deployed || {
          echo "Program verification failed after preload fallback (${PROGRAM_ID_EFFECTIVE})" >&2
          return 1
        }
      fi
      ;;
    *)
      echo "Unknown DEPLOY_MODE=${DEPLOY_MODE}; expected auto|cli|preload" >&2
      return 1
      ;;
  esac
}

bootstrap_failure_diagnostics() {
  echo "Bootstrap failed. Showing recent diagnostics:" >&2
  echo "--- bootstrap.log (tail) ---" >&2
  tail -n 80 "${LOG_DIR}/bootstrap.log" >&2 || true

  local txid
  txid="$(
    rg -o "txid: '[^']+'" "${LOG_DIR}/bootstrap.log" 2>/dev/null \
      | tail -n 1 \
      | sed -E "s/txid: '([^']+)'/\\1/"
  )"
  if [[ -n "${txid}" ]]; then
    echo "--- solana confirm -v ${txid} ---" >&2
    solana -u "${SOLANA_URL}" confirm -v "${txid}" >&2 || true
  fi
}

bootstrap_local_state() {
  cd "${MNG_DIR}"
  env \
    TS_NODE_TRANSPILE_ONLY="${TS_NODE_TRANSPILE_ONLY}" \
    CLUSTER_OVERRIDE=devnet \
    CLUSTER_URL_OVERRIDE="${SOLANA_URL}" \
    CTM_RELAYER_PROGRAM_ID="${PROGRAM_ID_EFFECTIVE}" \
    MB_PAYER_KEYPAIR="${MB_PAYER_KEYPAIR}" \
    EXECUTION_QUEUE_GROUP_NUM="${GROUP_NUM}" \
    PERP_MARKET_INDEX="${PERP_MARKET_INDEX}" \
    E2E_BUFFER_LAYOUT_PATH="${BUFFER_LAYOUT_PATH}" \
    E2E_MAKER_KEYPAIR_PATH="${MAKER_KEYPAIR_PATH}" \
    E2E_TAKER_KEYPAIR_PATH="${TAKER_KEYPAIR_PATH}" \
    E2E_LANE_CONFIG_PATH="${LANE_CONFIG_PATH}" \
    E2E_OUTPUT_CONFIG_PATH="${E2E_CONFIG_PATH}" \
    npm run -s execution-queue-local-perp-e2e-bootstrap >"${LOG_DIR}/bootstrap.log" 2>&1 || {
      bootstrap_failure_diagnostics
      return 1
    }
}

read_cfg_field() {
  local field="$1"
  node -p "require('${E2E_CONFIG_PATH}').${field}"
}

start_harness_and_relayer() {
  cd "${MNG_DIR}"
  if [[ ! -f "${E2E_CONFIG_PATH}" ]]; then
    echo "Missing config: ${E2E_CONFIG_PATH}" >&2
    exit 1
  fi
  local buffer_pk usdc_mint group_pk
  buffer_pk="$(read_cfg_field executionQueueBuffer || true)"
  usdc_mint="$(read_cfg_field usdcMint)"
  group_pk="$(read_cfg_field group)"
  local queue_pk
  queue_pk="$(read_cfg_field executionQueue)"
  if [[ -z "${buffer_pk}" ]]; then
    buffer_pk="${queue_pk}"
  fi

  setsid env \
    TS_NODE_TRANSPILE_ONLY="${TS_NODE_TRANSPILE_ONLY}" \
    CLUSTER_OVERRIDE=devnet \
    CLUSTER_URL_OVERRIDE="${SOLANA_URL}" \
    CONTINUUM_HARNESS_BIND_ADDR=127.0.0.1:9091 \
    CONTINUUM_HARNESS_MODE=local \
    CONTINUUM_HARNESS_PROGRAM_ID="${PROGRAM_ID_EFFECTIVE}" \
    CONTINUUM_HARNESS_EVENT_LOG_PATH="${CONTINUUM_EVENT_LOG_PATH}" \
    CONTINUUM_HARNESS_ENABLE_AIRDROP="${HARNESS_ENABLE_AIRDROP}" \
    CONTINUUM_HARNESS_USDC_MINT="${usdc_mint}" \
    CONTINUUM_HARNESS_AIRDROP_KEYPAIR="${MB_PAYER_KEYPAIR}" \
    CONTINUUM_HARNESS_GROUP_PK="${group_pk}" \
    CONTINUUM_HARNESS_AIRDROP_DEPOSIT_UI_AMOUNT="${HARNESS_AIRDROP_DEPOSIT_UI_AMOUNT}" \
    ./node_modules/.bin/ts-node ts/client/scripts/execution-queue/continuum-state-harness.ts \
    >"${LOG_DIR}/continuum-harness.log" 2>&1 < /dev/null &
  echo $! >"${HARNESS_PID_FILE}"
  wait_for_http_or_fail \
    "harness" \
    "http://127.0.0.1:9091/healthz" \
    "${HARNESS_START_TIMEOUT_SECS}" \
    "${HARNESS_PID_FILE}" \
    "${LOG_DIR}/continuum-harness.log"

  if [[ "${CTM_RELAYER_IMPL}" == "rust" ]]; then
    if [[ -x "${EXECUTION_ENGINE_BIN}" ]]; then
      setsid env \
        CLUSTER_OVERRIDE=devnet \
        CLUSTER_URL_OVERRIDE="${SOLANA_URL}" \
        CTM_RELAYER_PROGRAM_ID="${PROGRAM_ID_EFFECTIVE}" \
        CTM_RELAYER_BIND_ADDR=127.0.0.1:9090 \
        CTM_EXECUTION_ENGINE_HTTP_BIND_ADDR="${CTM_EXECUTION_ENGINE_HTTP_BIND_ADDR}" \
        CTM_RELAYER_PAYER_KEYPAIR="${CTM_RELAYER_PAYER_KEYPAIR}" \
        CTM_RELAYER_CTM_KEYPAIR="${CTM_RELAYER_CTM_KEYPAIR}" \
        CTM_RELAYER_EVENT_SINK_URL=http://127.0.0.1:9091/ingest/relay-intent \
        EXECUTION_QUEUE_BUFFER_PK="${buffer_pk}" \
        EXECUTION_QUEUE_GROUP_PK="${group_pk}" \
        EXECUTION_QUEUE_PK="${queue_pk}" \
        EXECUTION_QUEUE_CRANK_LANES_JSON_PATH="${RUN_DIR}/execution-queue-lanes-${GROUP_NUM}.json" \
        EXECUTION_QUEUE_CRANK_MAX_ITEMS=8 \
        EXECUTION_QUEUE_CRANK_INTERVAL_MS=250 \
        EXECUTION_QUEUE_CRANK_SKIP_PREFLIGHT=true \
        CTM_RELAYER_SEQUENCE_STATE_PATH="${RELAYER_SEQUENCE_STATE_PATH}" \
        CTM_RELAYER_MIN_EXECUTE_SLOT_OFFSET=1 \
        "${EXECUTION_ENGINE_BIN}" \
        >"${LOG_DIR}/ctm-relayer.log" 2>&1 < /dev/null &
    else
      setsid env \
        CLUSTER_OVERRIDE=devnet \
        CLUSTER_URL_OVERRIDE="${SOLANA_URL}" \
        CTM_RELAYER_PROGRAM_ID="${PROGRAM_ID_EFFECTIVE}" \
        CTM_RELAYER_BIND_ADDR=127.0.0.1:9090 \
        CTM_EXECUTION_ENGINE_HTTP_BIND_ADDR="${CTM_EXECUTION_ENGINE_HTTP_BIND_ADDR}" \
        CTM_RELAYER_PAYER_KEYPAIR="${CTM_RELAYER_PAYER_KEYPAIR}" \
        CTM_RELAYER_CTM_KEYPAIR="${CTM_RELAYER_CTM_KEYPAIR}" \
        CTM_RELAYER_EVENT_SINK_URL=http://127.0.0.1:9091/ingest/relay-intent \
        EXECUTION_QUEUE_BUFFER_PK="${buffer_pk}" \
        EXECUTION_QUEUE_GROUP_PK="${group_pk}" \
        EXECUTION_QUEUE_PK="${queue_pk}" \
        EXECUTION_QUEUE_CRANK_LANES_JSON_PATH="${RUN_DIR}/execution-queue-lanes-${GROUP_NUM}.json" \
        EXECUTION_QUEUE_CRANK_MAX_ITEMS=8 \
        EXECUTION_QUEUE_CRANK_INTERVAL_MS=250 \
        EXECUTION_QUEUE_CRANK_SKIP_PREFLIGHT=true \
        CTM_RELAYER_SEQUENCE_STATE_PATH="${RELAYER_SEQUENCE_STATE_PATH}" \
        CTM_RELAYER_MIN_EXECUTE_SLOT_OFFSET=1 \
        cargo run -p service-mango-execution-engine \
        >"${LOG_DIR}/ctm-relayer.log" 2>&1 < /dev/null &
    fi
  else
    setsid env \
      TS_NODE_TRANSPILE_ONLY="${TS_NODE_TRANSPILE_ONLY}" \
      CLUSTER_OVERRIDE=devnet \
      CLUSTER_URL_OVERRIDE="${SOLANA_URL}" \
      CTM_RELAYER_PROGRAM_ID="${PROGRAM_ID_EFFECTIVE}" \
      CTM_RELAYER_BIND_ADDR=127.0.0.1:9090 \
      CTM_RELAYER_PAYER_KEYPAIR="${CTM_RELAYER_PAYER_KEYPAIR}" \
      CTM_RELAYER_CTM_KEYPAIR="${CTM_RELAYER_CTM_KEYPAIR}" \
      CTM_RELAYER_EVENT_SINK_URL=http://127.0.0.1:9091/ingest/relay-intent \
      EXECUTION_QUEUE_BUFFER_PK="${buffer_pk}" \
      CTM_RELAYER_SEQUENCE_STATE_PATH="${RELAYER_SEQUENCE_STATE_PATH}" \
      CTM_RELAYER_MIN_EXECUTE_SLOT_OFFSET=1 \
      ./node_modules/.bin/ts-node ts/client/scripts/execution-queue/ctm-sequencer-relayer.ts \
      >"${LOG_DIR}/ctm-relayer.log" 2>&1 < /dev/null &
  fi
  echo $! >"${RELAYER_PID_FILE}"
  wait_for_port_or_fail \
    "relayer" \
    9090 \
    "${RELAYER_START_TIMEOUT_SECS}" \
    "${RELAYER_PID_FILE}" \
    "${LOG_DIR}/ctm-relayer.log"
  if [[ "${CTM_RELAYER_IMPL}" == "rust" ]]; then
    wait_for_http_or_fail \
      "execution-engine" \
      "http://${CTM_EXECUTION_ENGINE_HTTP_BIND_ADDR}/healthz" \
      "${RELAYER_START_TIMEOUT_SECS}" \
      "${RELAYER_PID_FILE}" \
      "${LOG_DIR}/ctm-relayer.log"
  fi
}

start_bridge() {
  cd "${MNG_DIR}"
  setsid env \
    TS_NODE_TRANSPILE_ONLY="${TS_NODE_TRANSPILE_ONLY}" \
    CLUSTER_OVERRIDE=devnet \
    CLUSTER_URL_OVERRIDE="${SOLANA_URL}" \
    CTM_RELAYER_ADDR=127.0.0.1:9090 \
    CTM_RELAYER_HTTP_BIND_ADDR=127.0.0.1:9092 \
    E2E_OUTPUT_CONFIG_PATH="${E2E_CONFIG_PATH}" \
    FERMI_TSDB_ENABLED=false \
    ./node_modules/.bin/ts-node ts/client/scripts/execution-queue/ctm-relayer-http-bridge.ts \
    >"${LOG_DIR}/ctm-relayer-http-bridge.log" 2>&1 < /dev/null &
  echo $! >"${BRIDGE_PID_FILE}"
  wait_for_http_or_fail \
    "bridge" \
    "http://127.0.0.1:9092/healthz" \
    "${BRIDGE_START_TIMEOUT_SECS}" \
    "${BRIDGE_PID_FILE}" \
    "${LOG_DIR}/ctm-relayer-http-bridge.log"
}

start_frontend() {
  cd "${FRONTEND_DIR}"
  setsid npm run dev -- --host "${FRONTEND_HOST}" --port "${FRONTEND_PORT}" \
    >"${FRONTEND_RUN_DIR}/frontend.log" 2>&1 < /dev/null &
  echo $! >"${FRONTEND_PID_FILE}"
  wait_for_http_or_fail \
    "frontend" \
    "http://${FRONTEND_HOST}:${FRONTEND_PORT}/" \
    "${FRONTEND_START_TIMEOUT_SECS}" \
    "${FRONTEND_PID_FILE}" \
    "${FRONTEND_RUN_DIR}/frontend.log"
}

smoke_checks() {
  local owner
  owner="${SMOKE_OWNER:-}"
  if [[ -z "${owner}" && -f "${E2E_CONFIG_PATH}" ]]; then
    owner="$(node -p "require('${E2E_CONFIG_PATH}').maker.owner || ''")"
  fi
  if [[ -z "${owner}" ]]; then
    owner="11111111111111111111111111111111"
  fi

  log "Smoke: harness direct"
  curl -fsS "http://127.0.0.1:9091/healthz" >/dev/null
  log "Smoke: harness via frontend proxy"
  curl -fsS "http://${FRONTEND_HOST}:${FRONTEND_PORT}/harness/healthz" >/dev/null
  log "Smoke: relay config via bridge proxy"
  curl -fsS \
    "http://${FRONTEND_HOST}:${FRONTEND_PORT}/bridge/relay/config?owner=${owner}" >/dev/null
  log "Smoke: harness reports airdrop enabled"
  curl -fsS "http://${FRONTEND_HOST}:${FRONTEND_PORT}/harness/healthz" | rg -q '"airdrop_enabled":true'
  log "Smoke: nginx https gateway"
  curl -kfsS "https://127.0.0.1/harness/healthz" >/dev/null
  curl -kfsS "https://127.0.0.1/bridge/healthz" >/dev/null
}

status_all() {
  echo "validator: $(pid_is_running "${VALIDATOR_PID_FILE}" && echo running || echo stopped)"
  echo "harness:   $(pid_is_running "${HARNESS_PID_FILE}" && echo running || echo stopped)"
  echo "relayer:   $(pid_is_running "${RELAYER_PID_FILE}" && echo running || echo stopped)"
  echo "bridge:    $(pid_is_running "${BRIDGE_PID_FILE}" && echo running || echo stopped)"
  echo "frontend:  $(pid_is_running "${FRONTEND_PID_FILE}" && echo running || echo stopped)"
  if command -v systemctl >/dev/null 2>&1; then
    if systemctl is-active --quiet nginx; then
      echo "nginx:    running"
    else
      echo "nginx:    stopped"
    fi
  else
    if ss -ltn | rg -q ':443[[:space:]]'; then
      echo "nginx:    running (port 443)"
    else
      echo "nginx:    stopped"
    fi
  fi
  echo "ports:"
  ss -ltn | rg ':(443|5173|8899|9090|9091|9092|9093)\b' || true
  echo "health:"
  if curl -fsS "http://127.0.0.1:9091/healthz" >/dev/null 2>&1; then
    echo "  harness: ok"
  else
    echo "  harness: fail"
  fi
  if curl -fsS "http://127.0.0.1:9092/healthz" >/dev/null 2>&1; then
    echo "  bridge:  ok"
  else
    echo "  bridge:  fail"
  fi
  if [[ "${CTM_RELAYER_IMPL}" == "rust" ]]; then
    if curl -fsS "http://${CTM_EXECUTION_ENGINE_HTTP_BIND_ADDR}/healthz" >/dev/null 2>&1; then
      echo "  execution_engine: ok"
    else
      echo "  execution_engine: fail"
    fi
  fi
  if curl -fsS "http://${FRONTEND_HOST}:${FRONTEND_PORT}/" >/dev/null 2>&1; then
    echo "  frontend: ok"
  else
    echo "  frontend: fail"
  fi
  if curl -kfsS "https://127.0.0.1/harness/healthz" >/dev/null 2>&1; then
    echo "  nginx_https: ok"
  else
    echo "  nginx_https: fail"
  fi
}

restart_all() {
  preflight_binaries
  preflight_program_identity
  stop_all
  fresh_cleanup
  start_validator regular
  deploy_or_preload_program
  bootstrap_local_state
  start_harness_and_relayer
  start_bridge
  start_frontend
  ensure_nginx_https_gateway
  smoke_checks
  status_all
}

case "${1:-restart}" in
  restart)
    restart_all
    ;;
  stop)
    stop_all
    ;;
  status)
    status_all
    ;;
  *)
    echo "Usage: $(basename "$0") [restart|stop|status]" >&2
    exit 1
    ;;
esac
