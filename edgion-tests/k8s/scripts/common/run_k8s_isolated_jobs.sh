#!/usr/bin/env bash

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/../../../.." && pwd)"
DEFAULT_MANIFEST="${SCRIPT_DIR}/k8s_isolated_jobs.tsv"
DEFAULT_JOB_TEMPLATE="${PROJECT_ROOT}/edgion-tests/k8s/kubernetes/isolated/job.template.yaml"

KUBECTL_BIN="${KUBECTL_BIN:-kubectl}"
MANIFEST="$DEFAULT_MANIFEST"
JOB_TEMPLATE="$DEFAULT_JOB_TEMPLATE"
IMAGE="${EDGION_ISOLATED_JOB_IMAGE:-}"
IMAGE_PULL_POLICY="${EDGION_ISOLATED_IMAGE_PULL_POLICY:-IfNotPresent}"
ARTIFACTS_DIR=""
TIMEOUT_OVERRIDE=""
DEADLINE_GRACE_SECONDS="${EDGION_ISOLATED_DEADLINE_GRACE_SECONDS:-30}"
KEEP_ON_FAILURE=false

JOB_IDS=()
JOB_SCRIPTS=()
JOB_TIMEOUTS=()
REQUESTED_IDS=()
RUN_NAMESPACE=""
NAMESPACE_CREATED=false
RUN_FAILED=0
CLEANUP_FINISHED=false

usage() {
  cat <<'EOF'
Usage: run_k8s_isolated_jobs.sh --image IMAGE [options]

Run canonical isolated integration scripts inside run-owned Kubernetes Jobs.

Options:
  --image IMAGE                 Dedicated isolated-test image
  --job ID                      Run one manifest-backed job; repeatable
  --manifest PATH               Isolated job manifest TSV
  --job-template PATH           Kubernetes Job template
  --timeout-seconds N           Override each selected job timeout
  --deadline-grace-seconds N    Kubernetes deadline grace after script timeout
  --artifacts-dir PATH          Host directory for logs and diagnostics
  --image-pull-policy POLICY    Always, IfNotPresent, or Never
  --keep-on-failure             Preserve the run namespace after a failure
  -h, --help                    Show this help

With no --job option, every job in the manifest is run sequentially.

Protocol:
  @@ISOLATED_RESULT ID PASS|FAIL rc
  @@ISOLATED_SUMMARY total=N failed=N infrastructure_failed=0|1
EOF
}

die() {
  echo "ERROR: $*" >&2
  exit 2
}

is_positive_integer() {
  case "$1" in
    ''|*[!0-9]*|0) return 1 ;;
    *) return 0 ;;
  esac
}

while [ "$#" -gt 0 ]; do
  case "$1" in
    --image)
      [ "$#" -ge 2 ] || die "--image requires a value"
      IMAGE="$2"
      shift 2
      ;;
    --job)
      [ "$#" -ge 2 ] || die "--job requires an id"
      REQUESTED_IDS+=("$2")
      shift 2
      ;;
    --manifest)
      [ "$#" -ge 2 ] || die "--manifest requires a path"
      MANIFEST="$2"
      shift 2
      ;;
    --job-template)
      [ "$#" -ge 2 ] || die "--job-template requires a path"
      JOB_TEMPLATE="$2"
      shift 2
      ;;
    --timeout-seconds)
      [ "$#" -ge 2 ] || die "--timeout-seconds requires a value"
      TIMEOUT_OVERRIDE="$2"
      shift 2
      ;;
    --deadline-grace-seconds)
      [ "$#" -ge 2 ] || die "--deadline-grace-seconds requires a value"
      DEADLINE_GRACE_SECONDS="$2"
      shift 2
      ;;
    --artifacts-dir)
      [ "$#" -ge 2 ] || die "--artifacts-dir requires a path"
      ARTIFACTS_DIR="$2"
      shift 2
      ;;
    --image-pull-policy)
      [ "$#" -ge 2 ] || die "--image-pull-policy requires a value"
      IMAGE_PULL_POLICY="$2"
      shift 2
      ;;
    --keep-on-failure)
      KEEP_ON_FAILURE=true
      shift
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *) die "unknown argument: $1" ;;
  esac
done

[ -n "$IMAGE" ] || die "--image is required"
printf '%s\n' "$IMAGE" | grep -Eq '^[A-Za-z0-9._/@:-]+$' \
  || die "image contains unsupported characters"
case "$IMAGE_PULL_POLICY" in
  Always|IfNotPresent|Never) ;;
  *) die "invalid image pull policy: ${IMAGE_PULL_POLICY}" ;;
esac
[ -f "$MANIFEST" ] || die "manifest not found: ${MANIFEST}"
[ -f "$JOB_TEMPLATE" ] || die "job template not found: ${JOB_TEMPLATE}"
if [ -n "$TIMEOUT_OVERRIDE" ]; then
  is_positive_integer "$TIMEOUT_OVERRIDE" || die "timeout must be a positive integer"
fi
case "$DEADLINE_GRACE_SECONDS" in
  ''|*[!0-9]*) die "deadline grace must be a non-negative integer" ;;
esac
command -v "$KUBECTL_BIN" >/dev/null 2>&1 || die "kubectl command not found: ${KUBECTL_BIN}"
"$KUBECTL_BIN" version --client >/dev/null 2>&1 || die "kubectl client preflight failed"

tab="$(printf '\t')"
while IFS="$tab" read -r scenario_id canonical_script timeout_seconds extra; do
  case "$scenario_id" in
    ''|'#'*) continue ;;
  esac
  [ -z "$extra" ] || die "manifest entry for ${scenario_id} has extra fields"
  printf '%s\n' "$scenario_id" | grep -Eq '^[A-Za-z][A-Za-z0-9_]*$' \
    || die "invalid scenario id in manifest: ${scenario_id}"
  printf '%s\n' "$canonical_script" \
    | grep -Eq '^edgion-tests/integration/scripts/integration/test_[a-z0-9_]+\.sh$' \
    || die "invalid canonical script for ${scenario_id}"
  is_positive_integer "$timeout_seconds" || die "invalid timeout for ${scenario_id}"
  for existing_id in "${JOB_IDS[@]-}"; do
    [ "$existing_id" != "$scenario_id" ] || die "duplicate manifest scenario: ${scenario_id}"
  done
  JOB_IDS+=("$scenario_id")
  JOB_SCRIPTS+=("$canonical_script")
  JOB_TIMEOUTS+=("$timeout_seconds")
done < "$MANIFEST"

[ "${#JOB_IDS[@]}" -gt 0 ] || die "manifest contains no jobs"
if [ "${#REQUESTED_IDS[@]}" -eq 0 ]; then
  REQUESTED_IDS=("${JOB_IDS[@]}")
fi

for requested_id in "${REQUESTED_IDS[@]}"; do
  found=false
  printf '%s\n' "$requested_id" | grep -Eq '^[A-Za-z][A-Za-z0-9_]*$' \
    || die "invalid requested scenario id: ${requested_id}"
  duplicate_count=0
  for other_id in "${REQUESTED_IDS[@]}"; do
    if [ "$other_id" = "$requested_id" ]; then
      duplicate_count=$((duplicate_count + 1))
    fi
  done
  [ "$duplicate_count" -eq 1 ] || die "duplicate requested scenario: ${requested_id}"
  for manifest_id in "${JOB_IDS[@]}"; do
    if [ "$manifest_id" = "$requested_id" ]; then
      found=true
      break
    fi
  done
  $found || die "scenario is not executable by this K8s helper: ${requested_id}"
done

run_stamp="$(date -u +%Y%m%d%H%M%S)"
run_id="iso-${run_stamp}-$$"
RUN_NAMESPACE="edgion-isolated-${run_stamp}-$$"
case "$RUN_NAMESPACE" in
  edgion-isolated-[a-zA-Z0-9-]*) ;;
  *) die "generated unsafe namespace: ${RUN_NAMESPACE}" ;;
esac

if [ -z "$ARTIFACTS_DIR" ]; then
  ARTIFACTS_DIR="${PROJECT_ROOT}/tmp/integration_testing/k8s-isolated-${run_stamp}-$$"
fi
mkdir -p "$ARTIFACTS_DIR"
echo "INFO: isolated run namespace=${RUN_NAMESPACE} artifacts=${ARTIFACTS_DIR}"

cleanup_namespace() {
  if $CLEANUP_FINISHED || ! $NAMESPACE_CREATED; then
    return 0
  fi
  if $KEEP_ON_FAILURE && [ "$RUN_FAILED" -ne 0 ]; then
    echo "INFO: preserving failed isolated namespace ${RUN_NAMESPACE}"
    CLEANUP_FINISHED=true
    return 0
  fi
  case "$RUN_NAMESPACE" in
    edgion-isolated-*) ;;
    *) echo "ERROR: refusing to delete unsafe namespace ${RUN_NAMESPACE}" >&2; return 1 ;;
  esac
  if "$KUBECTL_BIN" delete namespace "$RUN_NAMESPACE" --wait=false >/dev/null 2>&1; then
    CLEANUP_FINISHED=true
    return 0
  fi
  echo "ERROR: failed to delete run-owned namespace ${RUN_NAMESPACE}" >&2
  return 1
}

trap 'cleanup_namespace >/dev/null 2>&1 || true' EXIT INT TERM

"$KUBECTL_BIN" create namespace "$RUN_NAMESPACE" >/dev/null
NAMESPACE_CREATED=true
"$KUBECTL_BIN" label namespace "$RUN_NAMESPACE" \
  "edgion.io/test-run=${run_id}" \
  "app.kubernetes.io/managed-by=edgion-k8s-isolated-runner" >/dev/null

manifest_index_for() {
  local wanted="$1" index=0
  for manifest_id in "${JOB_IDS[@]}"; do
    if [ "$manifest_id" = "$wanted" ]; then
      printf '%s\n' "$index"
      return 0
    fi
    index=$((index + 1))
  done
  return 1
}

collect_failure_diagnostics() {
  local job_name="$1" pod_name="$2" prefix="$3"
  "$KUBECTL_BIN" get job "$job_name" -n "$RUN_NAMESPACE" -o yaml \
    > "${prefix}.job.yaml" 2>&1 || true
  if [ -n "$pod_name" ]; then
    "$KUBECTL_BIN" get pod "$pod_name" -n "$RUN_NAMESPACE" -o yaml \
      > "${prefix}.pod.yaml" 2>&1 || true
    "$KUBECTL_BIN" describe pod "$pod_name" -n "$RUN_NAMESPACE" \
      > "${prefix}.pod.describe.txt" 2>&1 || true
  fi
  "$KUBECTL_BIN" get events -n "$RUN_NAMESPACE" --sort-by=.lastTimestamp \
    > "${prefix}.events.txt" 2>&1 || true
}

failed_count=0
for scenario_id in "${REQUESTED_IDS[@]}"; do
  manifest_index="$(manifest_index_for "$scenario_id")"
  canonical_script="${JOB_SCRIPTS[$manifest_index]}"
  timeout_seconds="${JOB_TIMEOUTS[$manifest_index]}"
  if [ -n "$TIMEOUT_OVERRIDE" ]; then
    timeout_seconds="$TIMEOUT_OVERRIDE"
  fi
  active_deadline=$((timeout_seconds + DEADLINE_GRACE_SECONDS))
  job_suffix="$(printf '%s' "$scenario_id" | tr '[:upper:]_' '[:lower:]-')"
  job_name="isolated-${job_suffix}"
  rendered_manifest="${ARTIFACTS_DIR}/${scenario_id}.job.yaml"
  log_file="${ARTIFACTS_DIR}/${scenario_id}.log"
  diagnostic_prefix="${ARTIFACTS_DIR}/${scenario_id}"

  sed \
    -e "s|__JOB_NAME__|${job_name}|g" \
    -e "s|__NAMESPACE__|${RUN_NAMESPACE}|g" \
    -e "s|__RUN_ID__|${run_id}|g" \
    -e "s|__SCENARIO_ID__|${scenario_id}|g" \
    -e "s|__ACTIVE_DEADLINE_SECONDS__|${active_deadline}|g" \
    -e "s|__SCRIPT_TIMEOUT_SECONDS__|${timeout_seconds}|g" \
    -e "s|__CANONICAL_SCRIPT__|${canonical_script}|g" \
    -e "s|__IMAGE_PULL_POLICY__|${IMAGE_PULL_POLICY}|g" \
    -e "s|__IMAGE__|${IMAGE}|g" \
    "$JOB_TEMPLATE" > "$rendered_manifest"

  echo "INFO: starting isolated scenario ${scenario_id}"
  if ! "$KUBECTL_BIN" apply -f "$rendered_manifest" >/dev/null; then
    echo "ERROR: failed to create Job for ${scenario_id}" >&2
    printf '@@ISOLATED_RESULT %s FAIL 125\n' "$scenario_id"
    failed_count=$((failed_count + 1))
    RUN_FAILED=1
    continue
  fi

  wait_started="$(date +%s)"
  job_state=""
  while :; do
    succeeded="$("$KUBECTL_BIN" get job "$job_name" -n "$RUN_NAMESPACE" \
      -o jsonpath='{.status.succeeded}' 2>/dev/null || true)"
    failed="$("$KUBECTL_BIN" get job "$job_name" -n "$RUN_NAMESPACE" \
      -o jsonpath='{.status.failed}' 2>/dev/null || true)"
    case "$succeeded" in
      ''|*[!0-9]*) ;;
      *) if [ "$succeeded" -gt 0 ]; then job_state="succeeded"; break; fi ;;
    esac
    case "$failed" in
      ''|*[!0-9]*) ;;
      *) if [ "$failed" -gt 0 ]; then job_state="failed"; break; fi ;;
    esac
    wait_now="$(date +%s)"
    if [ $((wait_now - wait_started)) -ge "$active_deadline" ]; then
      job_state="timed_out"
      break
    fi
    sleep 2
  done

  pod_name="$("$KUBECTL_BIN" get pods -n "$RUN_NAMESPACE" \
    -l "job-name=${job_name}" -o jsonpath='{.items[0].metadata.name}' 2>/dev/null || true)"
  if ! "$KUBECTL_BIN" logs "job/${job_name}" -n "$RUN_NAMESPACE" > "$log_file" 2>&1; then
    echo "ERROR: failed to collect logs for ${scenario_id}" >> "$log_file"
  fi
  cat "$log_file"

  result_status="FAIL"
  result_rc=125
  if [ "$job_state" = "timed_out" ]; then
    result_rc=124
  else
    pod_rc=""
    if [ -n "$pod_name" ]; then
      pod_rc="$("$KUBECTL_BIN" get pod "$pod_name" -n "$RUN_NAMESPACE" \
        -o jsonpath='{.status.containerStatuses[0].state.terminated.exitCode}' 2>/dev/null || true)"
    fi
    marker_count="$(awk '$1 == "@@ISOLATED_CONTAINER_RESULT" { count += 1 } END { print count + 0 }' "$log_file")"
    marker_valid="$(awk -v id="$scenario_id" '
      $1 == "@@ISOLATED_CONTAINER_RESULT" {
        if (NF == 4 && $2 == id && ($3 == "PASS" || $3 == "FAIL") && $4 ~ /^[0-9]+$/) {
          print $3 " " $4
        }
      }
    ' "$log_file")"
    if [ "$marker_count" -eq 1 ] && [ -n "$marker_valid" ] \
      && printf '%s\n' "$pod_rc" | grep -Eq '^[0-9]+$'; then
      set -- $marker_valid
      marker_status="$1"
      marker_rc="$2"
      if [ "$marker_rc" -eq "$pod_rc" ] \
        && { [ "$marker_status" = "PASS" ] && [ "$marker_rc" -eq 0 ] && [ "$job_state" = "succeeded" ]; }; then
        result_status="PASS"
        result_rc=0
      elif [ "$marker_status" = "FAIL" ] && [ "$marker_rc" -ne 0 ] && [ "$job_state" = "failed" ]; then
        result_rc="$marker_rc"
      fi
    fi
  fi

  if [ "$result_status" = "PASS" ]; then
    printf '@@ISOLATED_RESULT %s PASS 0\n' "$scenario_id"
  else
    collect_failure_diagnostics "$job_name" "$pod_name" "$diagnostic_prefix"
    printf '@@ISOLATED_RESULT %s FAIL %s\n' "$scenario_id" "$result_rc"
    failed_count=$((failed_count + 1))
    RUN_FAILED=1
  fi
done

infrastructure_failed=0
if ! cleanup_namespace; then
  infrastructure_failed=1
  RUN_FAILED=1
fi
trap - EXIT INT TERM

printf '@@ISOLATED_SUMMARY total=%s failed=%s infrastructure_failed=%s\n' \
  "${#REQUESTED_IDS[@]}" "$failed_count" "$infrastructure_failed"

if [ "$RUN_FAILED" -ne 0 ]; then
  exit 1
fi
