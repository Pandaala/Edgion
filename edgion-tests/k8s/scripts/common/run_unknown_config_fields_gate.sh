#!/usr/bin/env bash

set -euo pipefail

CONTROLLER_URL="${CONTROLLER_URL:-http://edgion-controller.edgion-system.svc.cluster.local:12101}"
CONTROLLER_ADMIN_TOKEN="${EDGION_TEST_ADMIN_TOKEN:-integration-controller-admin-token-0123456789}"
TEST_CLIENT_NAMESPACE="${TEST_CLIENT_NAMESPACE:-edgion-test}"
TEST_CLIENT_SELECTOR="${TEST_CLIENT_SELECTOR:-app=edgion-test-client}"
TEST_CLIENT_CONTAINER="${TEST_CLIENT_CONTAINER:-test-client}"
TEST_CLIENT_POD="${TEST_CLIENT_POD:-}"
REQUEST_TIMEOUT_SECS="${REQUEST_TIMEOUT_SECS:-10}"

usage() {
  cat <<'EOF'
Usage: run_unknown_config_fields_gate.sh [options]

Query the authenticated, fleet-wide Controller unknown-fields diagnostic from
an in-cluster test-client Pod. The cumulative result must contain zero findings.

Options:
  --controller-url URL           Controller Admin API base URL
  --controller-token TOKEN       Controller test admin bearer token
  --test-client-namespace NS     Test-client namespace (default: edgion-test)
  --test-client-selector LABELS  Test-client Pod selector (default: app=edgion-test-client)
  --test-client-container NAME   Test-client container (default: test-client)
  --test-client-pod POD          Use this running test-client Pod instead of discovering one
  --request-timeout-secs N       Curl deadline (default: 10)
  -h, --help                     Show this help

Runtime stdout protocol:
  @@FINAL_GATE_RESULT UnknownConfigFields PASS|FAIL total=N|unknown items=N|unknown truncated=true|false|unknown
  @@FINAL_GATE_SUMMARY total=1 failed=0|1
EOF
}

error() {
  echo "ERROR: $*" >&2
}

emit_result() {
  local status="$1"
  local total="$2"
  local item_count="$3"
  local truncated="$4"
  local failed=1
  if [ "$status" = "PASS" ]; then
    failed=0
  fi
  printf '@@FINAL_GATE_RESULT UnknownConfigFields %s total=%s items=%s truncated=%s\n' \
    "$status" "$total" "$item_count" "$truncated"
  printf '@@FINAL_GATE_SUMMARY total=1 failed=%s\n' "$failed"
}

while [ "$#" -gt 0 ]; do
  case "$1" in
    --controller-url)
      [ "$#" -ge 2 ] || { error "--controller-url requires a value"; exit 2; }
      CONTROLLER_URL="$2"
      shift 2
      ;;
    --controller-token)
      [ "$#" -ge 2 ] || { error "--controller-token requires a value"; exit 2; }
      CONTROLLER_ADMIN_TOKEN="$2"
      shift 2
      ;;
    --test-client-namespace)
      [ "$#" -ge 2 ] || { error "--test-client-namespace requires a value"; exit 2; }
      TEST_CLIENT_NAMESPACE="$2"
      shift 2
      ;;
    --test-client-selector)
      [ "$#" -ge 2 ] || { error "--test-client-selector requires a value"; exit 2; }
      TEST_CLIENT_SELECTOR="$2"
      shift 2
      ;;
    --test-client-container)
      [ "$#" -ge 2 ] || { error "--test-client-container requires a value"; exit 2; }
      TEST_CLIENT_CONTAINER="$2"
      shift 2
      ;;
    --test-client-pod)
      [ "$#" -ge 2 ] || { error "--test-client-pod requires a value"; exit 2; }
      TEST_CLIENT_POD="$2"
      shift 2
      ;;
    --request-timeout-secs)
      [ "$#" -ge 2 ] || { error "--request-timeout-secs requires a value"; exit 2; }
      REQUEST_TIMEOUT_SECS="$2"
      shift 2
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      error "unknown option: $1"
      usage >&2
      exit 2
      ;;
  esac
done

case "$REQUEST_TIMEOUT_SECS" in
  ""|*[!0-9]*)
    error "--request-timeout-secs must be a positive integer"
    exit 2
    ;;
esac
if [ "$REQUEST_TIMEOUT_SECS" -eq 0 ]; then
  error "--request-timeout-secs must be a positive integer"
  exit 2
fi
if [ -z "$CONTROLLER_ADMIN_TOKEN" ]; then
  error "Controller admin token is unavailable"
  exit 2
fi
if ! command -v kubectl >/dev/null 2>&1; then
  error "kubectl not found in PATH"
  exit 2
fi
if ! command -v python3 >/dev/null 2>&1; then
  error "python3 not found in PATH"
  exit 2
fi

if [ -z "$TEST_CLIENT_POD" ]; then
  if ! TEST_CLIENT_POD=$(kubectl get pods \
    --namespace "$TEST_CLIENT_NAMESPACE" \
    --selector "$TEST_CLIENT_SELECTOR" \
    --field-selector status.phase=Running \
    --sort-by=.metadata.creationTimestamp \
    --output custom-columns=NAME:.metadata.name,DELETING:.metadata.deletionTimestamp \
    --no-headers 2>/dev/null \
    | awk '$2 == "<none>" {print $1}' \
    | tail -n 1); then
    error "Failed to discover a running test-client Pod"
    emit_result FAIL unknown unknown unknown
    exit 1
  fi
fi
if [ -z "$TEST_CLIENT_POD" ]; then
  error "No running, non-terminating test-client Pod found"
  emit_result FAIL unknown unknown unknown
  exit 1
fi

DIAGNOSTIC_URL="${CONTROLLER_URL%/}/api/v1/diagnostics/unknown-fields"
response=""
if ! response=$(kubectl exec \
  --namespace "$TEST_CLIENT_NAMESPACE" \
  --container "$TEST_CLIENT_CONTAINER" \
  "$TEST_CLIENT_POD" -- \
  curl --silent --show-error --max-time "$REQUEST_TIMEOUT_SECS" \
  --write-out $'\n%{http_code}' \
  --header "Authorization: Bearer ${CONTROLLER_ADMIN_TOKEN}" \
  "$DIAGNOSTIC_URL"); then
  error "Unknown-config-fields diagnostics endpoint is unreachable"
  emit_result FAIL unknown unknown unknown
  exit 1
fi

http_code=$(printf '%s\n' "$response" | tail -n 1)
body=$(printf '%s\n' "$response" | sed '$d')
if [ "$http_code" != "200" ]; then
  error "Diagnostics endpoint answered HTTP ${http_code} (expected 200): ${DIAGNOSTIC_URL}"
  emit_result FAIL unknown unknown unknown
  exit 1
fi
if [ -z "$body" ]; then
  error "Diagnostics endpoint returned HTTP 200 with an empty body"
  emit_result FAIL unknown unknown unknown
  exit 1
fi

report=""
if ! report=$(printf '%s' "$body" | python3 -c '
import json
import sys

try:
    doc = json.load(sys.stdin)
except Exception:
    sys.exit(1)
data = doc.get("data") if isinstance(doc, dict) else None
if not isinstance(doc, dict) or doc.get("success") is not True or not isinstance(data, dict):
    sys.exit(1)
total = data.get("total")
truncated = data.get("truncated")
items = data.get("items")
if isinstance(total, bool) or not isinstance(total, int) or total < 0:
    sys.exit(1)
if not isinstance(truncated, bool) or not isinstance(items, list):
    sys.exit(1)
if (truncated and total <= len(items)) or (not truncated and total != len(items)):
    sys.exit(1)
print(f"TOTAL={total}")
print(f"TRUNCATED={str(truncated).lower()}")
print(f"ITEM_COUNT={len(items)}")
for item in items:
    if not isinstance(item, dict):
        sys.exit(1)
    kind = item.get("kind")
    namespace = item.get("namespace")
    name = item.get("name")
    paths = item.get("paths")
    if not isinstance(kind, str) or not kind or not isinstance(namespace, str):
        sys.exit(1)
    if not isinstance(name, str) or not name or not isinstance(paths, list) or not paths:
        sys.exit(1)
    if any(not isinstance(path, str) or not path for path in paths):
        sys.exit(1)
    paths_text = ", ".join(paths)
    print(f"{kind}/{namespace}/{name} -> {paths_text}")
'); then
  error "Diagnostics endpoint returned HTTP 200 but an unparseable or unexpected body"
  emit_result FAIL unknown unknown unknown
  exit 1
fi
if [ -z "$report" ]; then
  error "Diagnostics endpoint returned an empty parsed report"
  emit_result FAIL unknown unknown unknown
  exit 1
fi

total=$(printf '%s\n' "$report" | sed -n 's/^TOTAL=//p' | head -n 1)
truncated=$(printf '%s\n' "$report" | sed -n 's/^TRUNCATED=//p' | head -n 1)
item_count=$(printf '%s\n' "$report" | sed -n 's/^ITEM_COUNT=//p' | head -n 1)
case "$total" in
  ""|*[!0-9]*)
    error "Diagnostics endpoint returned an invalid total"
    emit_result FAIL unknown unknown unknown
    exit 1
    ;;
esac
case "$item_count" in
  ""|*[!0-9]*)
    error "Diagnostics endpoint returned an invalid item count"
    emit_result FAIL "$total" unknown "$truncated"
    exit 1
    ;;
esac
case "$truncated" in
  true|false) ;;
  *)
    error "Diagnostics endpoint returned an invalid truncated flag"
    emit_result FAIL "$total" "$item_count" unknown
    exit 1
    ;;
esac

if [ "$total" -gt 0 ]; then
  error "Unknown config fields detected on ${total} resource(s); returned items=${item_count}, truncated=${truncated}"
  printf '%s\n' "$report" | tail -n +4 | while IFS= read -r finding; do
    [ -n "$finding" ] || continue
    error "  $finding"
  done
  emit_result FAIL "$total" "$item_count" "$truncated"
  exit 1
fi

emit_result PASS "$total" "$item_count" "$truncated"
