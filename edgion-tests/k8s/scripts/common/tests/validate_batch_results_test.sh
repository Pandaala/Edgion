#!/usr/bin/env bash

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
HELPER="${SCRIPT_DIR}/../validate_batch_results.sh"
TEMP_DIR="$(mktemp -d)"

cleanup() {
    rm -rf -- "$TEMP_DIR"
}
trap cleanup EXIT INT TERM

# shellcheck source=../validate_batch_results.sh
source "$HELPER"

EXPECTED="${TEMP_DIR}/expected.txt"
STDOUT_FILE="${TEMP_DIR}/stdout.txt"
ERROR_FILE="${TEMP_DIR}/error.txt"

cat > "$EXPECTED" <<'EOF'
# The validator ignores comments and whitespace.
 HTTPRoute | Basic
EdgionPlugins|JwtAuth
EOF

expect_valid() {
    local name="$1"
    local process_rc="$2"
    if ! edgion_validate_k8s_batch_results "$EXPECTED" "$STDOUT_FILE" "$process_rc" >/dev/null; then
        echo "ERROR: valid case failed: $name" >&2
        exit 1
    fi
}

expect_invalid() {
    local name="$1"
    local process_rc="$2"
    if edgion_validate_k8s_batch_results "$EXPECTED" "$STDOUT_FILE" "$process_rc" >"$ERROR_FILE" 2>&1; then
        echo "ERROR: invalid case passed: $name" >&2
        exit 1
    fi
}

cat > "$STDOUT_FILE" <<'EOF'
ordinary test output
@@SUITE_RESULT HTTPRoute|Basic PASS 0
@@SUITE_RESULT EdgionPlugins|JwtAuth PASS 0
@@BATCH_SUMMARY total=2 failed=0
EOF
expect_valid "all suites pass" 0
[ "$EDGION_K8S_BATCH_TOTAL" -eq 2 ]
[ "$EDGION_K8S_BATCH_PASSED" -eq 2 ]
[ "$EDGION_K8S_BATCH_FAILED" -eq 0 ]

cat > "$STDOUT_FILE" <<'EOF'
@@SUITE_RESULT HTTPRoute|Basic PASS 0
@@SUITE_RESULT EdgionPlugins|JwtAuth FAIL 7
@@BATCH_SUMMARY total=2 failed=1
EOF
expect_valid "one suite fails with a valid protocol" 1
[ "$EDGION_K8S_BATCH_TOTAL" -eq 2 ]
[ "$EDGION_K8S_BATCH_PASSED" -eq 1 ]
[ "$EDGION_K8S_BATCH_FAILED" -eq 1 ]

cat > "$STDOUT_FILE" <<'EOF'
@@SUITE_RESULT HTTPRoute|Basic PASS 0
@@SUITE_RESULT Unknown|Suite PASS 0
@@BATCH_SUMMARY total=2 failed=0
EOF
expect_invalid "unknown suite" 0

cat > "$STDOUT_FILE" <<'EOF'
@@SUITE_RESULT HTTPRoute|Basic PASS 0
@@SUITE_RESULT HTTPRoute|Basic PASS 0
@@BATCH_SUMMARY total=2 failed=0
EOF
expect_invalid "duplicate suite result" 0

cat > "$STDOUT_FILE" <<'EOF'
@@SUITE_RESULT HTTPRoute|Basic PASS 0
@@BATCH_SUMMARY total=1 failed=0
EOF
expect_invalid "missing suite result" 0

cat > "$STDOUT_FILE" <<'EOF'
@@SUITE_RESULT HTTPRoute|Basic SKIP 0
@@SUITE_RESULT EdgionPlugins|JwtAuth PASS 0
@@BATCH_SUMMARY total=2 failed=0
EOF
expect_invalid "invalid status" 0

cat > "$STDOUT_FILE" <<'EOF'
@@SUITE_RESULT HTTPRoute|Basic PASS 9
@@SUITE_RESULT EdgionPlugins|JwtAuth PASS 0
@@BATCH_SUMMARY total=2 failed=0
EOF
expect_invalid "pass with non-zero rc" 0

cat > "$STDOUT_FILE" <<'EOF'
@@SUITE_RESULT HTTPRoute|Basic FAIL 0
@@SUITE_RESULT EdgionPlugins|JwtAuth PASS 0
@@BATCH_SUMMARY total=2 failed=1
EOF
expect_invalid "fail with zero rc" 1

cat > "$STDOUT_FILE" <<'EOF'
@@SUITE_RESULT HTTPRoute|Basic FAIL error
@@SUITE_RESULT EdgionPlugins|JwtAuth PASS 0
@@BATCH_SUMMARY total=2 failed=1
EOF
expect_invalid "non-numeric suite rc" 1

cat > "$STDOUT_FILE" <<'EOF'
@@SUITE_RESULT HTTPRoute|Basic PASS 0
@@SUITE_RESULT EdgionPlugins|JwtAuth PASS 0
@@BATCH_SUMMARY total=2 failed=0
@@BATCH_SUMMARY total=2 failed=0
EOF
expect_invalid "duplicate summary" 0

cat > "$STDOUT_FILE" <<'EOF'
@@SUITE_RESULT HTTPRoute|Basic PASS 0
@@SUITE_RESULT EdgionPlugins|JwtAuth PASS 0
EOF
expect_invalid "missing summary" 0

cat > "$STDOUT_FILE" <<'EOF'
@@SUITE_RESULT HTTPRoute|Basic PASS 0
@@SUITE_RESULT EdgionPlugins|JwtAuth PASS 0
@@BATCH_SUMMARY total=3 failed=0
EOF
expect_invalid "summary total mismatch" 0

cat > "$STDOUT_FILE" <<'EOF'
@@SUITE_RESULT HTTPRoute|Basic FAIL 7
@@SUITE_RESULT EdgionPlugins|JwtAuth PASS 0
@@BATCH_SUMMARY total=2 failed=0
EOF
expect_invalid "summary failed mismatch" 1

cat > "$STDOUT_FILE" <<'EOF'
@@SUITE_RESULT HTTPRoute|Basic PASS 0
@@SUITE_RESULT EdgionPlugins|JwtAuth PASS 0
@@BATCH_SUMMARY total=2 failed=0
EOF
expect_invalid "non-zero process rc with no failures" 1

cat > "$STDOUT_FILE" <<'EOF'
@@SUITE_RESULT HTTPRoute|Basic FAIL 7
@@SUITE_RESULT EdgionPlugins|JwtAuth PASS 0
@@BATCH_SUMMARY total=2 failed=1
EOF
expect_invalid "zero process rc with failures" 0
expect_invalid "unexpected process rc with failures" 2
expect_invalid "non-numeric process rc" invalid

cat > "$STDOUT_FILE" <<'EOF'
@@SUITE_RESULT HTTPRoute|Basic PASS 0 extra
@@SUITE_RESULT EdgionPlugins|JwtAuth PASS 0
@@BATCH_SUMMARY total=2 failed=0
EOF
expect_invalid "malformed result tag" 0

cat > "$STDOUT_FILE" <<'EOF'
@@SUITE_RESULT HTTPRoute|Basic PASS 0
@@SUITE_RESULT EdgionPlugins|JwtAuth PASS 0
@@BATCH_SUMMARY failed=0 total=2
EOF
expect_invalid "malformed summary tag" 0

cat > "$EXPECTED" <<'EOF'
HTTPRoute|Basic
HTTPRoute|Basic
EOF
cat > "$STDOUT_FILE" <<'EOF'
@@SUITE_RESULT HTTPRoute|Basic PASS 0
@@BATCH_SUMMARY total=1 failed=0
EOF
expect_invalid "duplicate expected suite" 0

cat > "$EXPECTED" <<'EOF'
HTTPRoute|Basic
TLSRoute|Basic
EOF
cat > "$STDOUT_FILE" <<'EOF'
@@SUITE_START HTTPRoute|Basic
DEFERRED: DuplexBackpressure/Terminate upload: TLSRouteModeTerminate is not currently supported
@@SUITE_RESULT HTTPRoute|Basic PASS 0
@@SUITE_START TLSRoute|Basic
SKIPPED: suite `TLSRoute/Basic` requires TLSRouteModeTerminate, which is not currently supported
@@SUITE_RESULT TLSRoute|Basic SKIP 0
@@BATCH_SUMMARY total=2 failed=0 skipped=1
EOF
export EDGION_INTEGRATION_EXPLICIT_TARGET=0
edgion_validate_k8s_batch_results "$EXPECTED" "$STDOUT_FILE" 0 allow-tls-terminate-skip >/dev/null
[ "$EDGION_K8S_BATCH_EXECUTED" -eq 1 ]
[ "$EDGION_K8S_BATCH_SKIPPED" -eq 1 ]
[ "$EDGION_K8S_BATCH_PASSED" -eq 1 ]
expect_invalid "generic protocols reject TLS skip" 0
export EDGION_INTEGRATION_EXPLICIT_TARGET=1
if edgion_validate_k8s_batch_results "$EXPECTED" "$STDOUT_FILE" 0 allow-tls-terminate-skip >/dev/null 2>&1; then
    echo "ERROR: explicit target accepted skip" >&2
    exit 1
fi
export EDGION_INTEGRATION_EXPLICIT_TARGET=0
cp "$STDOUT_FILE" "${TEMP_DIR}/valid-skip.txt"
for mutation in missing duplicate wrong_suite wrong_reason nonzero summary forged_pass; do
    python3 - "${TEMP_DIR}/valid-skip.txt" "$STDOUT_FILE" "$mutation" <<'PY'
import pathlib, sys
text = pathlib.Path(sys.argv[1]).read_text()
reason = 'SKIPPED: suite `TLSRoute/Basic` requires TLSRouteModeTerminate, which is not currently supported\n'
changes = {
    'missing': (reason, ''),
    'duplicate': (reason, reason + reason),
    'wrong_suite': ('`TLSRoute/Basic`', '`TLSRoute/ProxyProtocol`'),
    'wrong_reason': ('requires TLSRouteModeTerminate', 'requires OtherCapability'),
    'nonzero': ('Basic SKIP 0', 'Basic SKIP 7'),
    'summary': ('skipped=1', 'skipped=0'),
    'forged_pass': ('Basic SKIP 0', 'Basic PASS 0'),
}
a, b = changes[sys.argv[3]]
pathlib.Path(sys.argv[2]).write_text(text.replace(a, b))
PY
    if edgion_validate_k8s_batch_results "$EXPECTED" "$STDOUT_FILE" 0 allow-tls-terminate-skip >/dev/null 2>&1; then
        echo "ERROR: invalid TLS skip accepted: $mutation" >&2
        exit 1
    fi
done

for suite in 'Dynamic|TlsRouteAttachments' 'Isolated|controller_auth' 'Final|CacheResidue'; do
    printf '%s\n' "$suite" > "$EXPECTED"
    printf '@@SUITE_RESULT %s SKIP 0\n@@BATCH_SUMMARY total=1 failed=0 skipped=1\n' "$suite" > "$STDOUT_FILE"
    expect_invalid "generic execution must reject skip: $suite" 0
done

echo "OK: K8s batch result protocol validation tests passed"
