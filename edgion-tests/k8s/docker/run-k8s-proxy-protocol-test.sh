#!/usr/bin/env bash

set -uo pipefail

scenario_id="${1:-}"
pod_ip="${POD_IP:-}"
failures=0

fail() {
  echo "ERROR: $*" >&2
  failures=$((failures + 1))
}

header_value() {
  local body="$1" name="$2"
  printf '%s' "$body" | python3 -c '
import json
import sys
try:
    data = json.load(sys.stdin)
except Exception:
    sys.exit(0)
print(data.get("headers", {}).get(sys.argv[1], ""))
' "$name"
}

wait_for_http() {
  local url="$1" attempts="${2:-60}" attempt=0
  while [ "$attempt" -lt "$attempts" ]; do
    if curl -fsS --connect-timeout 2 --max-time 3 "$url" >/dev/null 2>&1; then
      return 0
    fi
    attempt=$((attempt + 1))
    sleep 1
  done
  return 1
}

capture_wire() {
  local proxy_port="$1" version="$2" attempt=0 hex=""
  curl -sS --connect-timeout 2 --max-time 5 \
    "http://edgion-haproxy:${proxy_port}/" >/dev/null 2>&1 || true
  while [ "$attempt" -lt 20 ]; do
    hex="$(curl -fsS --connect-timeout 2 --max-time 3 \
      "http://edgion-wire-dump:18080/${version}" 2>/dev/null || true)"
    if [ -n "$hex" ]; then
      printf '%s\n' "$hex"
      return 0
    fi
    attempt=$((attempt + 1))
    sleep 1
  done
  return 1
}

assert_v2_wire() {
  local hex="$1" signature="0d0a0d0a000d0a515549540a"
  case "$hex" in
    "${signature}21"*) echo 'PASS: HAProxy emitted PROXY v2 signature and PROXY command' ;;
    *) fail "HAProxy v2 wire signature is invalid: ${hex}" ;;
  esac
}

assert_v1_wire() {
  local hex="$1"
  case "$hex" in
    50524f585920*) echo "PASS: HAProxy emitted PROXY v1 text signature" ;;
    *) fail "HAProxy v1 wire signature is invalid: ${hex}" ;;
  esac
}

request_body() {
  local port="$1"
  curl -fsS --connect-timeout 3 --max-time 10 "http://edgion-haproxy:${port}/headers" 2>/dev/null || true
}

assert_resolved_identity() {
  local label="$1" body="$2" xff xri
  if [ -z "$body" ]; then
    fail "${label}: no response through HAProxy"
    return
  fi
  xff="$(header_value "$body" x-forwarded-for)"
  xri="$(header_value "$body" x-real-ip)"
  case "$xff" in
    *"$pod_ip"*) ;;
    *) fail "${label}: X-Forwarded-For '${xff}' does not contain client Pod IP ${pod_ip}" ;;
  esac
  case "$xri" in
    *"$pod_ip"*) echo "PASS: ${label} resolved client Pod IP ${pod_ip}" ;;
    *) fail "${label}: X-Real-IP '${xri}' does not contain client Pod IP ${pod_ip}" ;;
  esac
}

run_inbound() {
  local v2_hex v1_hex body code curl_rc=0 plain_xff
  v2_hex="$(capture_wire 58442 v2 || true)"
  v1_hex="$(capture_wire 58443 v1 || true)"
  assert_v2_wire "$v2_hex"
  assert_v1_wire "$v1_hex"
  body="$(request_body 58440)"
  assert_resolved_identity 'HAProxy PPv2' "$body"
  body="$(request_body 58441)"
  assert_resolved_identity 'HAProxy PPv1' "$body"

  code="$(curl -sS -o /dev/null --connect-timeout 3 --max-time 5 -w '%{http_code}' \
    http://edgion-gateway:58430/headers 2>/dev/null)" || curl_rc=$?
  if [ "$code" = "200" ] || { [ "$curl_rc" -eq 0 ] && [ "$code" != "000" ]; }; then
    fail "plain request to mandatory PROXY listener returned code '${code}' rc ${curl_rc}"
  else
    echo 'PASS: mandatory PROXY listener rejected a headerless connection'
  fi

  body="$(curl -fsS --connect-timeout 3 --max-time 10 \
    http://edgion-gateway:58431/headers 2>/dev/null || true)"
  plain_xff="$(header_value "$body" x-forwarded-for)"
  case "$plain_xff" in
    *"$pod_ip"*) echo 'PASS: per-listener plain override preserved the direct client Pod identity' ;;
    *) fail "plain override X-Forwarded-For '${plain_xff}' does not contain ${pod_ip}" ;;
  esac
}

run_chain() {
  local v2_hex body xff remote_ip default_remote_ip
  v2_hex="$(capture_wire 58542 v2 || true)"
  assert_v2_wire "$v2_hex"
  body="$(request_body 58540)"
  if [ -z "$body" ]; then
    fail 'two-hop chain returned no response'
    return
  fi
  xff="$(header_value "$body" x-forwarded-for)"
  remote_ip="$(header_value "$body" x-edgion-remote-ip)"
  default_remote_ip="$(header_value "$body" x-real-ip)"
  if [ "$remote_ip" = "$pod_ip" ]; then
    echo "PASS: hop 2 resolved client Pod IP ${pod_ip}"
  else
    fail "hop 2 X-Edgion-Remote-IP is '${remote_ip}', expected ${pod_ip}"
  fi
  [ -z "$default_remote_ip" ] \
    || fail "custom RemoteIp header did not replace X-Real-IP: ${default_remote_ip}"
  case "$xff" in
    *"$pod_ip"*) ;;
    *) fail "chain X-Forwarded-For '${xff}' lost client Pod IP ${pod_ip}" ;;
  esac
  case "$xff" in
    *,*) echo "PASS: X-Forwarded-For records both gateway hops: ${xff}" ;;
    *) fail "chain X-Forwarded-For '${xff}' does not contain two hops" ;;
  esac
}

case "$scenario_id" in
  proxy_protocol_inbound|proxy_protocol_chain) ;;
  *)
    echo "ERROR: unsupported K8s PROXY protocol scenario: ${scenario_id}" >&2
    exit 2
    ;;
esac
case "$pod_ip" in
  ''|*[!0-9a-fA-F:.]*)
    echo "ERROR: POD_IP is missing or invalid: ${pod_ip}" >&2
    exit 2
    ;;
esac

wait_for_http http://edgion-controller:12153/ready 90 \
  || fail 'controller readiness deadline exceeded'
wait_for_http http://edgion-gateway:12161/health 90 \
  || fail 'gateway readiness deadline exceeded'
if [ "$scenario_id" = proxy_protocol_inbound ]; then
  wait_for_http http://edgion-haproxy:58440/headers 90 \
    || fail 'inbound PROXY listener convergence deadline exceeded'
  wait_for_http http://edgion-gateway:58431/headers 90 \
    || fail 'plain listener convergence deadline exceeded'
else
  wait_for_http http://edgion-haproxy:58540/headers 90 \
    || fail 'two-hop listener convergence deadline exceeded'
fi

if [ "$failures" -eq 0 ]; then
  if [ "$scenario_id" = "proxy_protocol_inbound" ]; then
    run_inbound
  else
    run_chain
  fi
fi

if [ "$failures" -eq 0 ]; then
  printf '@@ISOLATED_CONTAINER_RESULT %s PASS 0\n' "$scenario_id"
  exit 0
fi
printf '@@ISOLATED_CONTAINER_RESULT %s FAIL 1\n' "$scenario_id"
exit 1
