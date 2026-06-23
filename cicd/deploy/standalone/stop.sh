#!/usr/bin/env bash
# Stop Edgion controller and gateway processes.
#
# Usage:
#   ./stop.sh [--work-dir <path>]

set -euo pipefail

WORK_DIR="${WORK_DIR:-/usr/local/edgion}"

while [[ $# -gt 0 ]]; do
  case "$1" in
    --work-dir) WORK_DIR="${2:?Missing value}"; shift 2 ;;
    -h|--help) sed -n '/^# /s/^# //p' "$0"; exit 0 ;;
    *) echo "Unknown argument: $1" >&2; exit 1 ;;
  esac
done

stop_process() {
  local name="$1"
  local pidfile="${WORK_DIR}/${name}.pid"

  if [[ -f "${pidfile}" ]]; then
    local pid
    pid="$(cat "${pidfile}")"
    if kill -0 "${pid}" 2>/dev/null; then
      echo "Stopping ${name} (PID ${pid})..."
      kill "${pid}"
      for i in $(seq 1 10); do
        kill -0 "${pid}" 2>/dev/null || break
        sleep 1
      done
      if kill -0 "${pid}" 2>/dev/null; then
        echo "  Force killing ${name}..."
        kill -9 "${pid}" 2>/dev/null || true
      fi
      echo "  ${name} stopped."
    else
      echo "${name} is not running (stale PID file)."
    fi
    rm -f "${pidfile}"
  else
    echo "No PID file for ${name}, trying pkill..."
    pkill -f "edgion-${name}" 2>/dev/null || true
  fi
}

stop_process gateway
stop_process controller

echo "Edgion stopped."
