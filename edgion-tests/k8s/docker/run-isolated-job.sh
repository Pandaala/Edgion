#!/usr/bin/env bash

set -uo pipefail

scenario_id="${1:-}"
canonical_script="${2:-}"
timeout_seconds="${3:-}"
workspace="/workspace"

fail_protocol() {
  local message="$1"
  echo "ERROR: ${message}" >&2
  if [[ "$scenario_id" =~ ^[A-Za-z][A-Za-z0-9_]*$ ]]; then
    printf '@@ISOLATED_CONTAINER_RESULT %s FAIL 2\n' "$scenario_id"
  fi
  exit 2
}

[[ "$scenario_id" =~ ^[A-Za-z][A-Za-z0-9_]*$ ]] \
  || fail_protocol "invalid isolated scenario id"
[[ "$canonical_script" =~ ^edgion-tests/integration/scripts/integration/test_[a-z0-9_]+\.sh$ ]] \
  || fail_protocol "invalid canonical script path"
[[ "$timeout_seconds" =~ ^[1-9][0-9]*$ ]] \
  || fail_protocol "invalid timeout"

script_path="${workspace}/${canonical_script}"
[[ -f "$script_path" ]] || fail_protocol "canonical script is missing: ${canonical_script}"

echo "INFO: isolated scenario=${scenario_id} script=${canonical_script} timeout=${timeout_seconds}s"
if [[ "$scenario_id" == "oidc_egress" ]]; then
  resolver_library="${workspace}/target/debug/oidc_egress_resolver.so"
  [[ -f "$resolver_library" ]] || fail_protocol "OIDC resolver library is missing"
  resolver_addresses="$(LD_PRELOAD="$resolver_library" python3 - <<'PY'
import socket
print(" ".join(sorted({entry[4][0] for entry in socket.getaddrinfo("mixed.oidc-egress.test", 443)})))
PY
)" || fail_protocol "OIDC resolver library could not be loaded"
  [[ "$resolver_addresses" == *"127.0.0.1"* && "$resolver_addresses" == *"::"* ]] \
    || fail_protocol "OIDC resolver interception smoke check failed: ${resolver_addresses}"
  echo "INFO: OIDC resolver interception smoke check passed"
fi
set +e
timeout --signal=TERM --kill-after=15s "${timeout_seconds}s" bash "$script_path"
scenario_rc=$?
set -e

if [[ "$scenario_rc" -ne 0 ]]; then
  echo "INFO: bounded failure diagnostics from /workspace/tmp/integration_testing"
  if [[ -d "${workspace}/tmp/integration_testing" ]]; then
    find "${workspace}/tmp/integration_testing" -type f \
      \( -name '*.log' -o -name '*.status' -o -name '*.yaml' \) -size -2M -print \
      | sort \
      | while IFS= read -r diagnostic_file; do
          echo "===== ${diagnostic_file} ====="
          tail -n 120 "$diagnostic_file" 2>/dev/null || true
        done
  fi
  printf '@@ISOLATED_CONTAINER_RESULT %s FAIL %s\n' "$scenario_id" "$scenario_rc"
  exit "$scenario_rc"
fi

printf '@@ISOLATED_CONTAINER_RESULT %s PASS 0\n' "$scenario_id"
