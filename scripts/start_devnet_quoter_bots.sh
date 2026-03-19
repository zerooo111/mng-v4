#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
MNG_DIR="${ROOT_DIR}/mng-v4"
RUN_DIR="${MNG_DIR}/.devnet/run"
LOG_DIR="${MNG_DIR}/.devnet/logs"
PID_DIR="${RUN_DIR}/pids"
SCREEN_DIR="${SCREEN_DIR:-${ROOT_DIR}/.screen}"
USE_SCREEN="${USE_SCREEN:-true}"
SUPERVISOR_SCRIPT="${ROOT_DIR}/scripts/service_supervisor.sh"

GROUP_NUM="${GROUP_NUM:-9120}"
E2E_CONFIG_PATH="${E2E_CONFIG_PATH:-${RUN_DIR}/execution-queue-e2e-${GROUP_NUM}.json}"
QUOTER_BOTS_OUTPUT_PATH="${QUOTER_BOTS_OUTPUT_PATH:-${RUN_DIR}/quoter-bots-${GROUP_NUM}.json}"
QUOTER_ACTIVE_BOTS_PATH="${QUOTER_ACTIVE_BOTS_PATH:-${RUN_DIR}/quoter-bots-active-${GROUP_NUM}.json}"
QUOTER_PID_FILE="${PID_DIR}/quoter.pid"
QUOTER_LOG_FILE="${LOG_DIR}/quoter.log"
QUOTER_SESSION_NAME="${QUOTER_SESSION_NAME:-stagin4-devnet-quoter}"

QUOTER_BOT_COUNT="${QUOTER_BOT_COUNT:-4}"
QUOTER_INTERVAL_MS="${QUOTER_INTERVAL_MS:-2000}"
QUOTER_PRICE_RANGE_BPS="${QUOTER_PRICE_RANGE_BPS:-200}"
QUOTER_COINGECKO_ASSET_ID="${QUOTER_COINGECKO_ASSET_ID:-solana}"
QUOTER_COINGECKO_VS_CURRENCY="${QUOTER_COINGECKO_VS_CURRENCY:-usd}"
QUOTER_CANCEL_BEFORE_PLACE="${QUOTER_CANCEL_BEFORE_PLACE:-false}"
QUOTER_BOT_DISPATCH_MODE="${QUOTER_BOT_DISPATCH_MODE:-round-robin}"
QUOTER_LOG_EACH_ORDER="${QUOTER_LOG_EACH_ORDER:-true}"
QUOTER_ORDER_EXPIRY_SECS="${QUOTER_ORDER_EXPIRY_SECS:-30}"
QUOTER_CLOSE_POSITION_PROBABILITY_BPS="${QUOTER_CLOSE_POSITION_PROBABILITY_BPS:-1000}"
SUPERVISOR_RESTART_DELAY_SECS="${SUPERVISOR_RESTART_DELAY_SECS:-2}"

mkdir -p "${RUN_DIR}" "${LOG_DIR}" "${PID_DIR}"
if [[ "${USE_SCREEN}" == "true" ]]; then
  mkdir -p "${SCREEN_DIR}"
  chmod 700 "${SCREEN_DIR}"
fi

log() {
  echo "[quoter-launcher] $*"
}

pid_is_running() {
  if [[ ! -f "${QUOTER_PID_FILE}" ]]; then
    return 1
  fi
  local ref
  ref="$(cat "${QUOTER_PID_FILE}")"
  if [[ "${ref}" == screen:* ]]; then
    SCREENDIR="${SCREEN_DIR}" screen -S "${ref#screen:}" -Q select . >/dev/null 2>&1
    return $?
  fi
  [[ -n "${ref}" ]] && kill -0 "${ref}" 2>/dev/null
}

stop_quoter() {
  if pid_is_running; then
    local ref
    ref="$(cat "${QUOTER_PID_FILE}")"
    if [[ "${ref}" == screen:* ]]; then
      local session
      session="${ref#screen:}"
      log "stopping quoter screen=${session}"
      SCREENDIR="${SCREEN_DIR}" screen -S "${session}" -X quit >/dev/null 2>&1 || true
      for _ in $(seq 1 50); do
        if ! SCREENDIR="${SCREEN_DIR}" screen -S "${session}" -Q select . >/dev/null 2>&1; then
          break
        fi
        sleep 0.1
      done
    else
      local pid
      pid="${ref}"
      log "stopping quoter pid=${pid}"
      kill "${pid}" 2>/dev/null || true
      for _ in $(seq 1 50); do
        if ! kill -0 "${pid}" 2>/dev/null; then
          break
        fi
        sleep 0.1
      done
      kill -9 "${pid}" 2>/dev/null || true
    fi
  fi
  rm -f "${QUOTER_PID_FILE}"
}

read_cfg_field() {
  local field="$1"
  node -p "const cfg = require('${E2E_CONFIG_PATH}'); cfg.${field} || ''"
}

ensure_bot_specs() {
  if [[ ! -f "${E2E_CONFIG_PATH}" ]]; then
    echo "missing config: ${E2E_CONFIG_PATH}" >&2
    exit 1
  fi

  local need_provision=0
  if [[ ! -f "${QUOTER_BOTS_OUTPUT_PATH}" ]]; then
    need_provision=1
  else
    local existing_count
    existing_count="$(
      node -p "const fs = require('fs'); const bots = JSON.parse(fs.readFileSync('${QUOTER_BOTS_OUTPUT_PATH}', 'utf8')); bots.length"
    )"
    if (( existing_count < QUOTER_BOT_COUNT )); then
      need_provision=1
    fi
  fi

  if (( need_provision == 1 )); then
    log "provisioning ${QUOTER_BOT_COUNT} quoter bots"
    (
      cd "${MNG_DIR}"
      env \
        QUOTER_CONFIG_PATH="${E2E_CONFIG_PATH}" \
        QUOTER_BOTS_OUTPUT_PATH="${QUOTER_BOTS_OUTPUT_PATH}" \
        QUOTER_BOT_COUNT="${QUOTER_BOT_COUNT}" \
        npm run -s execution-queue-provision-quoter-bots
    )
  fi

  node -e "const fs = require('fs'); const bots = JSON.parse(fs.readFileSync(process.argv[1], 'utf8')); const count = Number(process.argv[3]); if (!Array.isArray(bots) || bots.length < count) { throw new Error('not enough provisioned bots'); } fs.writeFileSync(process.argv[2], JSON.stringify(bots.slice(0, count), null, 2));" \
    "${QUOTER_BOTS_OUTPUT_PATH}" \
    "${QUOTER_ACTIVE_BOTS_PATH}" \
    "${QUOTER_BOT_COUNT}"
}

start_quoter() {
  if pid_is_running; then
    log "quoter already running pid=$(cat "${QUOTER_PID_FILE}")"
    return 0
  fi

  ensure_bot_specs

  local cluster_url relayer_addr
  cluster_url="${CLUSTER_URL_OVERRIDE:-$(read_cfg_field clusterUrl)}"
  relayer_addr="${CTM_RELAYER_ADDR:-$(read_cfg_field relayer.bindAddr)}"

  if [[ -z "${cluster_url}" ]]; then
    echo "could not resolve clusterUrl from ${E2E_CONFIG_PATH}" >&2
    exit 1
  fi
  if [[ -z "${relayer_addr}" ]]; then
    relayer_addr="127.0.0.1:9090"
  fi

  log "starting quoter bots=${QUOTER_BOT_COUNT} intervalMs=${QUOTER_INTERVAL_MS} dispatch=${QUOTER_BOT_DISPATCH_MODE}"
  if [[ "${USE_SCREEN}" == "true" ]]; then
    local quoted_workdir quoted_logfile
    quoted_workdir="$(printf '%q' "${MNG_DIR}")"
    quoted_logfile="$(printf '%q' "${QUOTER_LOG_FILE}")"
    SCREENDIR="${SCREEN_DIR}" screen -S "${QUOTER_SESSION_NAME}" -X quit >/dev/null 2>&1 || true
    SCREENDIR="${SCREEN_DIR}" screen -DmS "${QUOTER_SESSION_NAME}" \
      bash -lc "cd ${quoted_workdir} && exec bash $(printf '%q' "${SUPERVISOR_SCRIPT}") --name quoter --workdir ${quoted_workdir} --log-file ${quoted_logfile} --restart-delay-secs $(printf '%q' "${SUPERVISOR_RESTART_DELAY_SECS}") -- env \
        QUOTER_CONFIG_PATH=$(printf '%q' "${E2E_CONFIG_PATH}") \
        QUOTER_BOTS_JSON_PATH=$(printf '%q' "${QUOTER_ACTIVE_BOTS_PATH}") \
        CLUSTER_URL_OVERRIDE=$(printf '%q' "${cluster_url}") \
        CTM_RELAYER_ADDR=$(printf '%q' "${relayer_addr}") \
        QUOTER_INTERVAL_MS=$(printf '%q' "${QUOTER_INTERVAL_MS}") \
        QUOTER_PRICE_RANGE_BPS=$(printf '%q' "${QUOTER_PRICE_RANGE_BPS}") \
        QUOTER_COINGECKO_ASSET_ID=$(printf '%q' "${QUOTER_COINGECKO_ASSET_ID}") \
        QUOTER_COINGECKO_VS_CURRENCY=$(printf '%q' "${QUOTER_COINGECKO_VS_CURRENCY}") \
        QUOTER_CANCEL_BEFORE_PLACE=$(printf '%q' "${QUOTER_CANCEL_BEFORE_PLACE}") \
        QUOTER_BOT_DISPATCH_MODE=$(printf '%q' "${QUOTER_BOT_DISPATCH_MODE}") \
        QUOTER_LOG_EACH_ORDER=$(printf '%q' "${QUOTER_LOG_EACH_ORDER}") \
        QUOTER_ORDER_EXPIRY_SECS=$(printf '%q' "${QUOTER_ORDER_EXPIRY_SECS}") \
        QUOTER_CLOSE_POSITION_PROBABILITY_BPS=$(printf '%q' "${QUOTER_CLOSE_POSITION_PROBABILITY_BPS}") \
        npm run -s execution-queue-random-sol-usdc-quoter" \
      >/dev/null 2>&1 &
    sleep 0.2
    echo "screen:${QUOTER_SESSION_NAME}" >"${QUOTER_PID_FILE}"
  else
    (
      cd "${MNG_DIR}"
      setsid env \
        QUOTER_CONFIG_PATH="${E2E_CONFIG_PATH}" \
        QUOTER_BOTS_JSON_PATH="${QUOTER_ACTIVE_BOTS_PATH}" \
        CLUSTER_URL_OVERRIDE="${cluster_url}" \
        CTM_RELAYER_ADDR="${relayer_addr}" \
        QUOTER_INTERVAL_MS="${QUOTER_INTERVAL_MS}" \
        QUOTER_PRICE_RANGE_BPS="${QUOTER_PRICE_RANGE_BPS}" \
        QUOTER_COINGECKO_ASSET_ID="${QUOTER_COINGECKO_ASSET_ID}" \
        QUOTER_COINGECKO_VS_CURRENCY="${QUOTER_COINGECKO_VS_CURRENCY}" \
        QUOTER_CANCEL_BEFORE_PLACE="${QUOTER_CANCEL_BEFORE_PLACE}" \
        QUOTER_BOT_DISPATCH_MODE="${QUOTER_BOT_DISPATCH_MODE}" \
        QUOTER_LOG_EACH_ORDER="${QUOTER_LOG_EACH_ORDER}" \
        QUOTER_ORDER_EXPIRY_SECS="${QUOTER_ORDER_EXPIRY_SECS}" \
        QUOTER_CLOSE_POSITION_PROBABILITY_BPS="${QUOTER_CLOSE_POSITION_PROBABILITY_BPS}" \
        npm run -s execution-queue-random-sol-usdc-quoter >"${QUOTER_LOG_FILE}" 2>&1 < /dev/null &
      echo $! >"${QUOTER_PID_FILE}"
    )
  fi

  log "quoter started ref=$(cat "${QUOTER_PID_FILE}") log=${QUOTER_LOG_FILE}"
}

status_quoter() {
  if pid_is_running; then
    echo "quoter: running ref=$(cat "${QUOTER_PID_FILE}")"
    echo "log: ${QUOTER_LOG_FILE}"
  else
    echo "quoter: stopped"
  fi
}

case "${1:-start}" in
  start)
    start_quoter
    ;;
  stop)
    stop_quoter
    ;;
  restart)
    stop_quoter
    start_quoter
    ;;
  status)
    status_quoter
    ;;
  *)
    echo "usage: $0 {start|stop|restart|status}" >&2
    exit 1
    ;;
esac
