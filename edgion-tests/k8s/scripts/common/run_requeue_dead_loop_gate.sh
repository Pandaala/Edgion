#!/usr/bin/env bash

set -euo pipefail

CONTROLLER_NAMESPACE="${CONTROLLER_NAMESPACE:-edgion-system}"
CONTROLLER_SELECTOR="${CONTROLLER_SELECTOR:-app=edgion-controller}"
CONTROLLER_CONTAINER="${CONTROLLER_CONTAINER:-edgion-controller}"
RUN_START_TIME="${RUN_START_TIME:-}"

CYCLE_PATTERN='Trigger cycle limit reached, dropping requeue'
DEPTH_PATTERN='Trigger chain depth limit reached, dropping requeue'

usage() {
  cat <<'EOF'
Usage: run_requeue_dead_loop_gate.sh --run-start-time RFC3339 [options]

Scan current logs for every Controller Pod, plus previous logs for containers
whose restartCount is non-zero. Every log request is bounded by --run-start-time.

Options:
  --run-start-time RFC3339       Required integration run start timestamp
  --controller-namespace NS      Controller namespace (default: edgion-system)
  --controller-selector LABELS   Controller Pod selector (default: app=edgion-controller)
  --controller-container NAME    Controller container (default: edgion-controller)
  -h, --help                     Show this help

Runtime stdout protocol:
  @@FINAL_GATE_RESULT RequeueDeadLoop PASS|FAIL pods=N streams=N matches=N
  @@FINAL_GATE_SUMMARY total=1 failed=0|1
EOF
}

error() {
  echo "ERROR: $*" >&2
}

emit_result() {
  local status="$1"
  local pod_count="$2"
  local stream_count="$3"
  local match_count="$4"
  local failed=1
  if [ "$status" = "PASS" ]; then
    failed=0
  fi
  printf '@@FINAL_GATE_RESULT RequeueDeadLoop %s pods=%s streams=%s matches=%s\n' \
    "$status" "$pod_count" "$stream_count" "$match_count"
  printf '@@FINAL_GATE_SUMMARY total=1 failed=%s\n' "$failed"
}

while [ "$#" -gt 0 ]; do
  case "$1" in
    --run-start-time)
      [ "$#" -ge 2 ] || { error "--run-start-time requires a value"; exit 2; }
      RUN_START_TIME="$2"
      shift 2
      ;;
    --controller-namespace)
      [ "$#" -ge 2 ] || { error "--controller-namespace requires a value"; exit 2; }
      CONTROLLER_NAMESPACE="$2"
      shift 2
      ;;
    --controller-selector)
      [ "$#" -ge 2 ] || { error "--controller-selector requires a value"; exit 2; }
      CONTROLLER_SELECTOR="$2"
      shift 2
      ;;
    --controller-container)
      [ "$#" -ge 2 ] || { error "--controller-container requires a value"; exit 2; }
      CONTROLLER_CONTAINER="$2"
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

if [ -z "$RUN_START_TIME" ]; then
  error "--run-start-time is required"
  exit 2
fi
if ! printf '%s\n' "$RUN_START_TIME" \
  | grep -Eq '^[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}:[0-9]{2}:[0-9]{2}([.][0-9]+)?(Z|[+-][0-9]{2}:[0-9]{2})$'; then
  error "--run-start-time must be an RFC3339 timestamp with a timezone"
  exit 2
fi
if ! command -v kubectl >/dev/null 2>&1; then
  error "kubectl not found in PATH"
  exit 2
fi
if ! command -v jq >/dev/null 2>&1; then
  error "jq not found in PATH"
  exit 2
fi

TEMP_DIR="$(mktemp -d /tmp/edgion-requeue-gate.XXXXXX)"
cleanup() {
  rm -rf "$TEMP_DIR"
}
trap cleanup EXIT INT TERM

pod_json=""
if ! pod_json=$(kubectl get pods \
  --namespace "$CONTROLLER_NAMESPACE" \
  --selector "$CONTROLLER_SELECTOR" \
  --output json); then
  error "Failed to list Controller Pods"
  emit_result FAIL 0 0 0
  exit 1
fi
if ! pod_count=$(printf '%s\n' "$pod_json" | jq -er '
  if .apiVersion == "v1"
     and (.kind == "List" or .kind == "PodList")
     and (.items | type) == "array"
     and all(.items[]; .apiVersion == "v1" and .kind == "Pod") then
    (.items | length)
  else
    error("not a Kubernetes Pod list")
  end
'); then
  error "Controller Pod inventory is not a valid Kubernetes Pod list"
  emit_result FAIL 0 0 0
  exit 1
fi
if [ "$pod_count" -eq 0 ]; then
  error "No Controller Pods found"
  emit_result FAIL 0 0 0
  exit 1
fi

POD_ROWS="${TEMP_DIR}/pods.tsv"
if ! printf '%s\n' "$pod_json" | jq -er --arg container "$CONTROLLER_CONTAINER" '
  .items[]
  | .metadata.name as $pod
  | [.status.containerStatuses[]? | select(.name == $container)] as $statuses
  | if ($pod | type) != "string" or ($pod | length) == 0 then
      error("Pod has no name")
    elif ($statuses | length) != 1 then
      error("Controller container status is missing or duplicated")
    elif ($statuses[0].restartCount | type) != "number"
         or ($statuses[0].restartCount | floor) != $statuses[0].restartCount
         or $statuses[0].restartCount < 0 then
      error("Controller restartCount is invalid")
    else
      [$pod, ($statuses[0].restartCount | tostring)] | @tsv
    end
' > "$POD_ROWS"; then
  error "Controller Pod inventory has missing or invalid container status"
  emit_result FAIL "$pod_count" 0 0
  exit 1
fi
if [ "$(wc -l < "$POD_ROWS" | tr -d '[:space:]')" -ne "$pod_count" ]; then
  error "Controller Pod accounting mismatch"
  emit_result FAIL "$pod_count" 0 0
  exit 1
fi

stream_count=0
match_count=0
failed=false
while IFS="$(printf '\t')" read -r pod restart_count; do
  streams="current"
  if [ "$restart_count" -gt 0 ]; then
    streams="current previous"
  fi
  for stream in $streams; do
    log_file="${TEMP_DIR}/${pod}.${stream}.log"
    previous_arg=""
    if [ "$stream" = "previous" ]; then
      previous_arg="--previous"
    fi

    if [ -n "$previous_arg" ]; then
      if ! kubectl logs \
        --namespace "$CONTROLLER_NAMESPACE" \
        --container "$CONTROLLER_CONTAINER" \
        --since-time "$RUN_START_TIME" \
        --previous \
        "$pod" > "$log_file"; then
        error "Failed to retrieve ${stream} Controller logs for Pod ${pod}"
        failed=true
        continue
      fi
    else
      if ! kubectl logs \
        --namespace "$CONTROLLER_NAMESPACE" \
        --container "$CONTROLLER_CONTAINER" \
        --since-time "$RUN_START_TIME" \
        "$pod" > "$log_file"; then
        error "Failed to retrieve ${stream} Controller logs for Pod ${pod}"
        failed=true
        continue
      fi
    fi
    stream_count=$((stream_count + 1))

    stream_matches=0
    grep_rc=0
    if stream_matches=$(grep -F -c \
      -e "$CYCLE_PATTERN" \
      -e "$DEPTH_PATTERN" \
      "$log_file"); then
      grep_rc=0
    else
      grep_rc=$?
    fi
    if [ "$grep_rc" -gt 1 ]; then
      error "Failed to scan ${stream} Controller logs for Pod ${pod}"
      failed=true
      continue
    fi
    if [ "$stream_matches" -gt 0 ]; then
      match_count=$((match_count + stream_matches))
      error "Requeue dead-loop pattern found in ${stream} Controller logs for Pod ${pod}: ${stream_matches} matching line(s)"
      grep -F \
        -e "$CYCLE_PATTERN" \
        -e "$DEPTH_PATTERN" \
        "$log_file" | head -20 >&2 || true
      failed=true
    fi
  done
done < "$POD_ROWS"

if [ "$failed" = "true" ]; then
  emit_result FAIL "$pod_count" "$stream_count" "$match_count"
  exit 1
fi

emit_result PASS "$pod_count" "$stream_count" "$match_count"
