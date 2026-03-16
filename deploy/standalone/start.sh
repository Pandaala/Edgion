#!/usr/bin/env bash
# Start Edgion controller and gateway in standalone (file-system) mode.
#
# Usage:
#   ./start.sh [OPTIONS]
#
# Options:
#   --work-dir <path>       Working directory (default: /usr/local/edgion)
#   --controller-config     Controller TOML config path
#   --gateway-config        Gateway TOML config path
#   --conf-dir <path>       Gateway API YAML config directory
#   --foreground            Run in foreground (don't daemonize)
#
# The script expects edgion-controller and edgion-gateway binaries in PATH
# or in the working directory's bin/ folder.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
WORK_DIR="${WORK_DIR:-/usr/local/edgion}"
CONTROLLER_CONFIG="${SCRIPT_DIR}/edgion-controller.toml"
GATEWAY_CONFIG="${SCRIPT_DIR}/edgion-gateway.toml"
CONF_DIR="${SCRIPT_DIR}/conf"
FOREGROUND=false

while [[ $# -gt 0 ]]; do
  case "$1" in
    -h|--help) sed -n '/^# /s/^# //p' "$0"; exit 0 ;;
    --work-dir) WORK_DIR="${2:?Missing value}"; shift 2 ;;
    --controller-config) CONTROLLER_CONFIG="${2:?Missing value}"; shift 2 ;;
    --gateway-config) GATEWAY_CONFIG="${2:?Missing value}"; shift 2 ;;
    --conf-dir) CONF_DIR="${2:?Missing value}"; shift 2 ;;
    --foreground) FOREGROUND=true; shift ;;
    *) echo "Unknown argument: $1" >&2; exit 1 ;;
  esac
done

find_binary() {
  local name="$1"
  if command -v "${name}" >/dev/null 2>&1; then
    echo "${name}"
  elif [[ -x "${WORK_DIR}/bin/${name}" ]]; then
    echo "${WORK_DIR}/bin/${name}"
  elif [[ -x "./target/release/${name}" ]]; then
    echo "./target/release/${name}"
  elif [[ -x "./target/debug/${name}" ]]; then
    echo "./target/debug/${name}"
  else
    echo "Binary not found: ${name}" >&2
    echo "Install it to PATH, ${WORK_DIR}/bin/, or build with: cargo build --release" >&2
    exit 1
  fi
}

CONTROLLER_BIN="$(find_binary edgion-controller)"
GATEWAY_BIN="$(find_binary edgion-gateway)"

mkdir -p "${WORK_DIR}/logs"

echo "═══════════════════════════════════════════════════"
echo "  Edgion Gateway — Standalone"
echo "═══════════════════════════════════════════════════"
echo "  Work dir:     ${WORK_DIR}"
echo "  Controller:   ${CONTROLLER_BIN}"
echo "  Gateway:      ${GATEWAY_BIN}"
echo "  Config dir:   ${CONF_DIR}"
echo "═══════════════════════════════════════════════════"

echo "Starting controller..."
if [[ "${FOREGROUND}" == true ]]; then
  "${CONTROLLER_BIN}" \
    --config-file "${CONTROLLER_CONFIG}" \
    --work-dir "${WORK_DIR}" \
    --conf-dir "${CONF_DIR}" &
  CONTROLLER_PID=$!
else
  "${CONTROLLER_BIN}" \
    --config-file "${CONTROLLER_CONFIG}" \
    --work-dir "${WORK_DIR}" \
    --conf-dir "${CONF_DIR}" \
    > "${WORK_DIR}/logs/controller.log" 2>&1 &
  CONTROLLER_PID=$!
fi
echo "  Controller PID: ${CONTROLLER_PID}"

echo "Waiting for controller to be ready..."
for i in $(seq 1 30); do
  if curl -sf http://127.0.0.1:5800/health >/dev/null 2>&1; then
    echo "  Controller is ready."
    break
  fi
  if [[ $i -eq 30 ]]; then
    echo "  Controller failed to start within 30s." >&2
    kill "${CONTROLLER_PID}" 2>/dev/null || true
    exit 1
  fi
  sleep 1
done

echo "Starting gateway..."
if [[ "${FOREGROUND}" == true ]]; then
  "${GATEWAY_BIN}" \
    --config-file "${GATEWAY_CONFIG}" \
    --work-dir "${WORK_DIR}" &
  GATEWAY_PID=$!
else
  "${GATEWAY_BIN}" \
    --config-file "${GATEWAY_CONFIG}" \
    --work-dir "${WORK_DIR}" \
    > "${WORK_DIR}/logs/gateway.log" 2>&1 &
  GATEWAY_PID=$!
fi
echo "  Gateway PID: ${GATEWAY_PID}"

echo ""
echo "Edgion is running."
echo "  Controller admin: http://127.0.0.1:5800"
echo "  Gateway admin:    http://127.0.0.1:5900"
echo "  Gateway HTTP:     http://127.0.0.1:80"
echo ""
echo "PIDs: controller=${CONTROLLER_PID}, gateway=${GATEWAY_PID}"
echo "${CONTROLLER_PID}" > "${WORK_DIR}/controller.pid"
echo "${GATEWAY_PID}" > "${WORK_DIR}/gateway.pid"

if [[ "${FOREGROUND}" == true ]]; then
  echo "Running in foreground. Press Ctrl+C to stop."
  trap 'kill ${CONTROLLER_PID} ${GATEWAY_PID} 2>/dev/null; exit 0' INT TERM
  wait
fi
