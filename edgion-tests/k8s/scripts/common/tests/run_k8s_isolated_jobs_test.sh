#!/usr/bin/env bash

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
HELPER="${SCRIPT_DIR}/../run_k8s_isolated_jobs.sh"
JOB_TEMPLATE="${SCRIPT_DIR}/../../../kubernetes/isolated/job.template.yaml"
TEST_ROOT="$(mktemp -d)"
trap 'rm -rf "$TEST_ROOT"' EXIT

FAKE_KUBECTL="${TEST_ROOT}/kubectl"
MANIFEST="${TEST_ROOT}/jobs.tsv"
MUTATIONS="${TEST_ROOT}/mutations.log"

cat > "$MANIFEST" <<'EOF'
# scenario_id	canonical_script	timeout_seconds
controller_auth	edgion-tests/integration/scripts/integration/test_controller_no_auth.sh	5
EOF

cat > "$FAKE_KUBECTL" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail

mode="${FAKE_MODE:?}"
state_dir="${FAKE_STATE_DIR:?}"
mutations="${FAKE_MUTATIONS:?}"
command_name="${1:-}"
shift || true

joined=" $* "
case "$command_name" in
  version)
    echo 'Client Version: v1.33.0'
    ;;
  create)
    echo "create $*" >> "$mutations"
    ;;
  label)
    echo "label $*" >> "$mutations"
    ;;
  apply)
    echo "apply $*" >> "$mutations"
    manifest=""
    while [ "$#" -gt 0 ]; do
      if [ "$1" = "-f" ]; then manifest="$2"; break; fi
      shift
    done
    awk '/^  name:/ { gsub(/[" ]/, "", $2); print $2; exit }' "$manifest" > "${state_dir}/job_name"
    ;;
  get)
    kind="${1:-}"
    name="${2:-}"
    case "$kind" in
      job)
        if [[ "$joined" == *"jsonpath={.status.succeeded}"* ]]; then
          case "$mode" in success|missing|duplicate|wrong_id|bad_status|bad_rc|cleanup_fail) printf '1' ;; *) : ;; esac
        elif [[ "$joined" == *"jsonpath={.status.failed}"* ]]; then
          case "$mode" in script_failure|mismatch) printf '1' ;; *) : ;; esac
        else
          echo 'apiVersion: batch/v1'
          echo 'kind: Job'
        fi
        ;;
      pods)
        job_name="$(cat "${state_dir}/job_name")"
        printf 'pod-%s' "$job_name"
        ;;
      pod)
        if [[ "$joined" == *"jsonpath={.status.containerStatuses[0].state.terminated.exitCode}"* ]]; then
          case "$mode" in script_failure|mismatch) printf '7' ;; *) printf '0' ;; esac
        else
          echo 'apiVersion: v1'
          echo 'kind: Pod'
        fi
        ;;
      events)
        echo 'No resources found'
        ;;
    esac
    ;;
  logs)
    case "$mode" in
      success|cleanup_fail) echo '@@ISOLATED_CONTAINER_RESULT controller_auth PASS 0' ;;
      script_failure) echo '@@ISOLATED_CONTAINER_RESULT controller_auth FAIL 7' ;;
      missing) echo 'canonical script exited without a result marker' ;;
      duplicate)
        echo '@@ISOLATED_CONTAINER_RESULT controller_auth PASS 0'
        echo '@@ISOLATED_CONTAINER_RESULT controller_auth PASS 0'
        ;;
      mismatch) echo '@@ISOLATED_CONTAINER_RESULT controller_auth PASS 0' ;;
      wrong_id) echo '@@ISOLATED_CONTAINER_RESULT gateway_admin_auth PASS 0' ;;
      bad_status) echo '@@ISOLATED_CONTAINER_RESULT controller_auth SKIP 0' ;;
      bad_rc) echo '@@ISOLATED_CONTAINER_RESULT controller_auth PASS zero' ;;
    esac
    ;;
  describe)
    echo 'fake pod diagnostics'
    ;;
  delete)
    echo "delete $*" >> "$mutations"
    if [ "$mode" = "cleanup_fail" ]; then exit 1; fi
    ;;
  *)
    echo "unexpected fake kubectl command: ${command_name} $*" >&2
    exit 90
    ;;
esac
EOF
chmod +x "$FAKE_KUBECTL"

run_case() {
  local mode="$1" expected_rc="$2" expected_result="$3" expected_summary="$4"
  local case_dir="${TEST_ROOT}/${mode}"
  mkdir -p "$case_dir"
  : > "$MUTATIONS"
  set +e
  FAKE_MODE="$mode" \
  FAKE_STATE_DIR="$case_dir" \
  FAKE_MUTATIONS="$MUTATIONS" \
  KUBECTL_BIN="$FAKE_KUBECTL" \
    "$HELPER" \
      --image edgion-isolated:test \
      --manifest "$MANIFEST" \
      --job-template "$JOB_TEMPLATE" \
      --job controller_auth \
      --artifacts-dir "${case_dir}/artifacts" \
      --timeout-seconds 1 \
      --deadline-grace-seconds 0 \
      > "${case_dir}/output.log" 2>&1
  local actual_rc=$?
  set -e
  [ "$actual_rc" -eq "$expected_rc" ]
  grep -qx "$expected_result" "${case_dir}/output.log"
  grep -qx "$expected_summary" "${case_dir}/output.log"
  grep -q '^delete namespace edgion-isolated-' "$MUTATIONS"
}

run_case success 0 \
  '@@ISOLATED_RESULT controller_auth PASS 0' \
  '@@ISOLATED_SUMMARY total=1 failed=0 infrastructure_failed=0'
run_case script_failure 1 \
  '@@ISOLATED_RESULT controller_auth FAIL 7' \
  '@@ISOLATED_SUMMARY total=1 failed=1 infrastructure_failed=0'
run_case missing 1 \
  '@@ISOLATED_RESULT controller_auth FAIL 125' \
  '@@ISOLATED_SUMMARY total=1 failed=1 infrastructure_failed=0'
run_case duplicate 1 \
  '@@ISOLATED_RESULT controller_auth FAIL 125' \
  '@@ISOLATED_SUMMARY total=1 failed=1 infrastructure_failed=0'
run_case mismatch 1 \
  '@@ISOLATED_RESULT controller_auth FAIL 125' \
  '@@ISOLATED_SUMMARY total=1 failed=1 infrastructure_failed=0'
run_case wrong_id 1 \
  '@@ISOLATED_RESULT controller_auth FAIL 125' \
  '@@ISOLATED_SUMMARY total=1 failed=1 infrastructure_failed=0'
run_case bad_status 1 \
  '@@ISOLATED_RESULT controller_auth FAIL 125' \
  '@@ISOLATED_SUMMARY total=1 failed=1 infrastructure_failed=0'
run_case bad_rc 1 \
  '@@ISOLATED_RESULT controller_auth FAIL 125' \
  '@@ISOLATED_SUMMARY total=1 failed=1 infrastructure_failed=0'
run_case timeout 1 \
  '@@ISOLATED_RESULT controller_auth FAIL 124' \
  '@@ISOLATED_SUMMARY total=1 failed=1 infrastructure_failed=0'

cleanup_dir="${TEST_ROOT}/cleanup_fail"
mkdir -p "$cleanup_dir"
: > "$MUTATIONS"
set +e
FAKE_MODE=cleanup_fail \
FAKE_STATE_DIR="$cleanup_dir" \
FAKE_MUTATIONS="$MUTATIONS" \
KUBECTL_BIN="$FAKE_KUBECTL" \
  "$HELPER" \
    --image edgion-isolated:test \
    --manifest "$MANIFEST" \
    --job-template "$JOB_TEMPLATE" \
    --job controller_auth \
    --artifacts-dir "${cleanup_dir}/artifacts" \
    --timeout-seconds 1 \
    --deadline-grace-seconds 0 \
    > "${cleanup_dir}/output.log" 2>&1
cleanup_rc=$?
set -e
[ "$cleanup_rc" -eq 1 ]
grep -qx '@@ISOLATED_RESULT controller_auth PASS 0' "${cleanup_dir}/output.log"
grep -qx '@@ISOLATED_SUMMARY total=1 failed=0 infrastructure_failed=1' "${cleanup_dir}/output.log"

: > "$MUTATIONS"
unsupported_dir="${TEST_ROOT}/unsupported"
mkdir -p "$unsupported_dir"
set +e
FAKE_MODE=success \
FAKE_STATE_DIR="$unsupported_dir" \
FAKE_MUTATIONS="$MUTATIONS" \
KUBECTL_BIN="$FAKE_KUBECTL" \
  "$HELPER" \
    --image edgion-isolated:test \
    --manifest "$MANIFEST" \
    --job-template "$JOB_TEMPLATE" \
    --job proxy_protocol_inbound \
    --artifacts-dir "${unsupported_dir}/artifacts" \
    > "${unsupported_dir}/output.log" 2>&1
unsupported_rc=$?
set -e
[ "$unsupported_rc" -eq 2 ]
grep -q 'scenario is not executable by this K8s helper: proxy_protocol_inbound' "${unsupported_dir}/output.log"
[ ! -s "$MUTATIONS" ]

echo 'run_k8s_isolated_jobs_test.sh: PASS'
