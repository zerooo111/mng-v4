#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
LOCALNET_DIR="${ROOT_DIR}/.localnet"
LEDGER_DIR="${LOCALNET_DIR}/ledger"
LOG_DIR="${LOCALNET_DIR}/logs"
RUN_DIR="${LOCALNET_DIR}/run"
PID_DIR="${RUN_DIR}/pids"

PROGRAM_KEYPAIR="${ROOT_DIR}/target/deploy/mango_v4-keypair.json"
PROGRAM_SO="${ROOT_DIR}/target/deploy/mango_v4.so"
PROGRAM_ID="${PROGRAM_ID:-9nNhSkcxYFujiydpuuhVttUYBqYJQmxCzjrBofBvmutF}"

SOLANA_URL="${SOLANA_URL:-http://127.0.0.1:8899}"
GROUP_NUM="${GROUP_NUM:-9120}"
PERP_MARKET_INDEX="${PERP_MARKET_INDEX:-0}"
MB_PAYER_KEYPAIR="${MB_PAYER_KEYPAIR:-/home/ec2-user/.config/solana/id.json}"
CTM_RELAYER_PAYER_KEYPAIR="${CTM_RELAYER_PAYER_KEYPAIR:-${MB_PAYER_KEYPAIR}}"
CTM_RELAYER_CTM_KEYPAIR="${CTM_RELAYER_CTM_KEYPAIR:-${MB_PAYER_KEYPAIR}}"
CTM_RELAYER_BIND_ADDR="${CTM_RELAYER_BIND_ADDR:-127.0.0.1:9090}"
HARNESS_BIND_ADDR="${HARNESS_BIND_ADDR:-127.0.0.1:9091}"
RESET_VALIDATOR="${RESET_VALIDATOR:-1}"
BUILD_SBF="${BUILD_SBF:-0}"
# Some services (notably the harness) can take >60s to become ready on cold starts.
STARTUP_WAIT_TRIES="${STARTUP_WAIT_TRIES:-180}"

BUFFER_LAYOUT_PATH="${RUN_DIR}/execution-queue-buffer-${GROUP_NUM}.json"
MAKER_KEYPAIR_PATH="${RUN_DIR}/execution-queue-maker-${GROUP_NUM}.json"
TAKER_KEYPAIR_PATH="${RUN_DIR}/execution-queue-taker-${GROUP_NUM}.json"
LANE_CONFIG_PATH="${RUN_DIR}/execution-queue-lanes-${GROUP_NUM}.json"
E2E_OUTPUT_CONFIG_PATH="${RUN_DIR}/execution-queue-e2e-${GROUP_NUM}.json"
RELAYER_SEQUENCE_STATE_PATH="${RUN_DIR}/ctm-sequences-${GROUP_NUM}.json"
CONTINUUM_EVENT_LOG_PATH="${RUN_DIR}/continuum-harness-${GROUP_NUM}.jsonl"

VALIDATOR_PID_FILE="${PID_DIR}/validator.pid"
RELAYER_PID_FILE="${PID_DIR}/relayer.pid"
HARNESS_PID_FILE="${PID_DIR}/harness.pid"
CRANKER_PID_FILE="${PID_DIR}/cranker.pid"

SOLANA_116_BIN_DIR="${SOLANA_116_BIN_DIR:-/home/ec2-user/.local/solana-1.16.7-release/bin}"
if [[ -d "${SOLANA_116_BIN_DIR}" ]]; then
  export PATH="$HOME/.cargo/bin:${SOLANA_116_BIN_DIR}:/home/ec2-user/.local/share/solana/install/active_release/bin:$PATH"
else
  export PATH="$HOME/.cargo/bin:/home/ec2-user/.local/share/solana/install/active_release/bin:$PATH"
fi

mkdir -p "${LEDGER_DIR}" "${LOG_DIR}" "${RUN_DIR}" "${PID_DIR}"
cd "${ROOT_DIR}"

pid_is_running() {
  local pid_file="$1"
  if [[ ! -f "${pid_file}" ]]; then
    return 1
  fi
  local pid
  pid="$(cat "${pid_file}")"
  if [[ -z "${pid}" ]]; then
    return 1
  fi
  kill -0 "${pid}" 2>/dev/null
}

stop_if_running() {
  local pid_file="$1"
  local name="$2"
  if pid_is_running "${pid_file}"; then
    local pid
    pid="$(cat "${pid_file}")"
    echo "Stopping ${name} (pid ${pid})"
    kill "${pid}" 2>/dev/null || true
    for _ in $(seq 1 50); do
      if ! kill -0 "${pid}" 2>/dev/null; then
        break
      fi
      sleep 0.1
    done
    if kill -0 "${pid}" 2>/dev/null; then
      kill -9 "${pid}" 2>/dev/null || true
    fi
  fi
  rm -f "${pid_file}"
}

wait_for_rpc() {
  local max_tries="${STARTUP_WAIT_TRIES}"
  for _ in $(seq 1 "${max_tries}"); do
    if curl -s "${SOLANA_URL}" \
      -H 'Content-Type: application/json' \
      -d '{"jsonrpc":"2.0","id":1,"method":"getHealth"}' \
      | rg -q '"result":"ok"'; then
      return 0
    fi
    sleep 1
  done
  echo "RPC did not become healthy at ${SOLANA_URL}" >&2
  return 1
}

wait_for_http_ok() {
  local url="$1"
  local max_tries="${STARTUP_WAIT_TRIES}"
  for _ in $(seq 1 "${max_tries}"); do
    if curl -fsS "${url}" >/dev/null 2>&1; then
      return 0
    fi
    sleep 1
  done
  echo "HTTP endpoint did not become ready: ${url}" >&2
  return 1
}

bind_port() {
  local bind_addr="$1"
  echo "${bind_addr##*:}"
}

port_is_listening() {
  local port="$1"
  ss -ltn "( sport = :${port} )" | rg -q ":${port}[[:space:]]"
}

wait_for_port_listen() {
  local port="$1"
  local max_tries="${STARTUP_WAIT_TRIES}"
  for _ in $(seq 1 "${max_tries}"); do
    if port_is_listening "${port}"; then
      return 0
    fi
    sleep 1
  done
  echo "Port ${port} did not start listening" >&2
  return 1
}

wait_for_harness_health() {
  local url="$1"
  local max_tries="${STARTUP_WAIT_TRIES}"
  for _ in $(seq 1 "${max_tries}"); do
    if curl -s "${url}" | rg -q '"ok"[[:space:]]*:[[:space:]]*true'; then
      return 0
    fi
    sleep 1
  done
  echo "Harness health check failed at ${url}" >&2
  return 1
}

ensure_port_free_or_owned() {
  local port="$1"
  local pid_file="$2"
  if port_is_listening "${port}" && ! pid_is_running "${pid_file}"; then
    echo "Port ${port} is already in use by another process; refusing to start." >&2
    return 1
  fi
  return 0
}

build_if_requested() {
  if [[ "${BUILD_SBF}" == "1" ]]; then
    cargo build-sbf --manifest-path "${ROOT_DIR}/programs/mango-v4/Cargo.toml" --features enable-gpl
  fi
}

deploy_program() {
  if [[ ! -f "${PROGRAM_SO}" || ! -f "${PROGRAM_KEYPAIR}" ]]; then
    echo "Missing deploy artifacts (${PROGRAM_SO} / ${PROGRAM_KEYPAIR})" >&2
    exit 1
  fi
  solana -u "${SOLANA_URL}" program deploy --program-id "${PROGRAM_KEYPAIR}" "${PROGRAM_SO}"
}

bootstrap_local_state() {
  env \
    CLUSTER_OVERRIDE=devnet \
    CLUSTER_URL_OVERRIDE="${SOLANA_URL}" \
    CTM_RELAYER_PROGRAM_ID="${PROGRAM_ID}" \
    MB_PAYER_KEYPAIR="${MB_PAYER_KEYPAIR}" \
    EXECUTION_QUEUE_GROUP_NUM="${GROUP_NUM}" \
    PERP_MARKET_INDEX="${PERP_MARKET_INDEX}" \
    E2E_BUFFER_LAYOUT_PATH="${BUFFER_LAYOUT_PATH}" \
    E2E_MAKER_KEYPAIR_PATH="${MAKER_KEYPAIR_PATH}" \
    E2E_TAKER_KEYPAIR_PATH="${TAKER_KEYPAIR_PATH}" \
    E2E_LANE_CONFIG_PATH="${LANE_CONFIG_PATH}" \
    E2E_OUTPUT_CONFIG_PATH="${E2E_OUTPUT_CONFIG_PATH}" \
    npm run -s execution-queue-local-perp-e2e-bootstrap \
    >"${LOG_DIR}/bootstrap.log" 2>&1
}

read_cfg_field() {
  local field="$1"
  node -p "require('${E2E_OUTPUT_CONFIG_PATH}').${field}"
}

start_validator() {
  local rpc_port ws_port
  rpc_port="$(bind_port "${SOLANA_URL}")"
  ws_port=8900

  if pid_is_running "${VALIDATOR_PID_FILE}"; then
    echo "Validator already running (pid $(cat "${VALIDATOR_PID_FILE}"))"
    return 0
  fi
  ensure_port_free_or_owned "${rpc_port}" "${VALIDATOR_PID_FILE}"
  ensure_port_free_or_owned "${ws_port}" "${VALIDATOR_PID_FILE}"
  if [[ "${RESET_VALIDATOR}" == "1" ]]; then
    rm -rf "${LEDGER_DIR}"
    mkdir -p "${LEDGER_DIR}"
  fi
  local reset_flag=()
  if [[ "${RESET_VALIDATOR}" == "1" ]]; then
    reset_flag+=(--reset)
  fi
  setsid solana-test-validator \
    --ledger "${LEDGER_DIR}" \
    "${reset_flag[@]}" \
    >"${LOG_DIR}/validator.log" 2>&1 &
  echo $! >"${VALIDATOR_PID_FILE}"
  wait_for_rpc
}

start_harness() {
  local harness_port
  local usdc_mint
  local group_pk
  harness_port="$(bind_port "${HARNESS_BIND_ADDR}")"
  usdc_mint="$(read_cfg_field usdcMint)"
  group_pk="$(read_cfg_field group)"

  if pid_is_running "${HARNESS_PID_FILE}"; then
    echo "Harness already running (pid $(cat "${HARNESS_PID_FILE}"))"
    return 0
  fi
  ensure_port_free_or_owned "${harness_port}" "${HARNESS_PID_FILE}"
  setsid env \
    CLUSTER_OVERRIDE=devnet \
    CLUSTER_URL_OVERRIDE="${SOLANA_URL}" \
    CONTINUUM_HARNESS_BIND_ADDR="${HARNESS_BIND_ADDR}" \
    CONTINUUM_HARNESS_MODE=local \
    CONTINUUM_HARNESS_PROGRAM_ID="${PROGRAM_ID}" \
    CONTINUUM_HARNESS_EVENT_LOG_PATH="${CONTINUUM_EVENT_LOG_PATH}" \
    CONTINUUM_HARNESS_ENABLE_AIRDROP=true \
    CONTINUUM_HARNESS_USDC_MINT="${usdc_mint}" \
    CONTINUUM_HARNESS_AIRDROP_KEYPAIR="${MB_PAYER_KEYPAIR}" \
    CONTINUUM_HARNESS_GROUP_PK="${group_pk}" \
    CONTINUUM_HARNESS_AIRDROP_DEPOSIT_UI_AMOUNT=1000 \
    ./node_modules/.bin/ts-node ts/client/scripts/execution-queue/continuum-state-harness.ts \
    >"${LOG_DIR}/continuum-harness.log" 2>&1 < /dev/null &
  echo $! >"${HARNESS_PID_FILE}"
  wait_for_port_listen "${harness_port}"
  wait_for_harness_health "http://${HARNESS_BIND_ADDR}/healthz"
}

start_relayer() {
  local relayer_port
  relayer_port="$(bind_port "${CTM_RELAYER_BIND_ADDR}")"

  if pid_is_running "${RELAYER_PID_FILE}"; then
    echo "Relayer already running (pid $(cat "${RELAYER_PID_FILE}"))"
    return 0
  fi
  ensure_port_free_or_owned "${relayer_port}" "${RELAYER_PID_FILE}"
  local buffer_pk
  buffer_pk="$(read_cfg_field executionQueueBuffer)"
  setsid env \
    CLUSTER_OVERRIDE=devnet \
    CLUSTER_URL_OVERRIDE="${SOLANA_URL}" \
    CTM_RELAYER_PROGRAM_ID="${PROGRAM_ID}" \
    CTM_RELAYER_BIND_ADDR="${CTM_RELAYER_BIND_ADDR}" \
    CTM_RELAYER_PAYER_KEYPAIR="${CTM_RELAYER_PAYER_KEYPAIR}" \
    CTM_RELAYER_CTM_KEYPAIR="${CTM_RELAYER_CTM_KEYPAIR}" \
    CTM_RELAYER_EVENT_SINK_URL="http://${HARNESS_BIND_ADDR}/ingest/relay-intent" \
    EXECUTION_QUEUE_BUFFER_PK="${buffer_pk}" \
    CTM_RELAYER_SEQUENCE_STATE_PATH="${RELAYER_SEQUENCE_STATE_PATH}" \
    CTM_RELAYER_MIN_EXECUTE_SLOT_OFFSET=1 \
    ./node_modules/.bin/ts-node ts/client/scripts/execution-queue/ctm-sequencer-relayer.ts \
    >"${LOG_DIR}/ctm-relayer.log" 2>&1 < /dev/null &
  echo $! >"${RELAYER_PID_FILE}"
  wait_for_port_listen "${relayer_port}"
}

start_cranker() {
  if pid_is_running "${CRANKER_PID_FILE}"; then
    echo "Cranker already running (pid $(cat "${CRANKER_PID_FILE}"))"
    return 0
  fi
  local group_pk queue_pk buffer_pk
  group_pk="$(read_cfg_field group)"
  queue_pk="$(read_cfg_field executionQueue)"
  buffer_pk="$(read_cfg_field executionQueueBuffer)"
  setsid env \
    CLUSTER_OVERRIDE=devnet \
    CLUSTER_URL_OVERRIDE="${SOLANA_URL}" \
    EXECUTION_QUEUE_GROUP_PK="${group_pk}" \
    EXECUTION_QUEUE_PK="${queue_pk}" \
    EXECUTION_QUEUE_BUFFER_PK="${buffer_pk}" \
    EXECUTION_QUEUE_PROGRAM_ID="${PROGRAM_ID}" \
    EXECUTION_QUEUE_CRANKER_KEYPAIR="${MB_PAYER_KEYPAIR}" \
    EXECUTION_QUEUE_CRANK_LANES_JSON_PATH="${LANE_CONFIG_PATH}" \
    EXECUTION_QUEUE_CRANK_MAX_ITEMS=8 \
    EXECUTION_QUEUE_CRANK_INTERVAL_MS=1000 \
    ./node_modules/.bin/ts-node ts/client/scripts/execution-queue/execution-queue-cranker.ts \
    >"${LOG_DIR}/execution-queue-cranker.log" 2>&1 < /dev/null &
  echo $! >"${CRANKER_PID_FILE}"
}

run_e2e() {
  env \
    CLUSTER_OVERRIDE=devnet \
    CLUSTER_URL_OVERRIDE="${SOLANA_URL}" \
    CTM_RELAYER_ADDR="${CTM_RELAYER_BIND_ADDR}" \
    E2E_OUTPUT_CONFIG_PATH="${E2E_OUTPUT_CONFIG_PATH}" \
    E2E_MAKER_MAX_QUOTE_QTY=1000 \
    E2E_TAKER_MAX_QUOTE_QTY=1000 \
    npm run -s execution-queue-local-perp-e2e-run
}

start_all() {
  start_validator
  build_if_requested
  deploy_program
  bootstrap_local_state
  start_harness
  start_relayer
  start_cranker
  echo "Local stack started:"
  echo "  validator rpc: ${SOLANA_URL}"
  echo "  relayer grpc:  ${CTM_RELAYER_BIND_ADDR}"
  echo "  harness http:  ${HARNESS_BIND_ADDR}"
  echo "  e2e config:    ${E2E_OUTPUT_CONFIG_PATH}"
  echo "  logs dir:      ${LOG_DIR}"
}

stop_all() {
  stop_if_running "${CRANKER_PID_FILE}" "cranker"
  stop_if_running "${RELAYER_PID_FILE}" "relayer"
  stop_if_running "${HARNESS_PID_FILE}" "harness"
  stop_if_running "${VALIDATOR_PID_FILE}" "validator"
}

status_all() {
  local names=("validator" "harness" "relayer" "cranker")
  local pid_files=(
    "${VALIDATOR_PID_FILE}"
    "${HARNESS_PID_FILE}"
    "${RELAYER_PID_FILE}"
    "${CRANKER_PID_FILE}"
  )
  for i in "${!names[@]}"; do
    if pid_is_running "${pid_files[$i]}"; then
      echo "${names[$i]}: running (pid $(cat "${pid_files[$i]}"))"
    else
      echo "${names[$i]}: stopped"
    fi
  done
  echo "rpc health:"
  curl -s "${SOLANA_URL}" \
    -H 'Content-Type: application/json' \
    -d '{"jsonrpc":"2.0","id":1,"method":"getHealth"}' || true
  echo
}

usage() {
  cat <<EOF
Usage: $(basename "$0") <start|stop|status|restart|run-e2e>

Environment overrides:
  GROUP_NUM=${GROUP_NUM}
  SOLANA_URL=${SOLANA_URL}
  PROGRAM_ID=${PROGRAM_ID}
  MB_PAYER_KEYPAIR=${MB_PAYER_KEYPAIR}
  CTM_RELAYER_BIND_ADDR=${CTM_RELAYER_BIND_ADDR}
  HARNESS_BIND_ADDR=${HARNESS_BIND_ADDR}
  RESET_VALIDATOR=${RESET_VALIDATOR}
  BUILD_SBF=${BUILD_SBF}
EOF
}

case "${1:-}" in
  start)
    start_all
    ;;
  stop)
    stop_all
    ;;
  status)
    status_all
    ;;
  restart)
    stop_all
    start_all
    ;;
  run-e2e)
    run_e2e
    ;;
  *)
    usage
    exit 1
    ;;
esac
