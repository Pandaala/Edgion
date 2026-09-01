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

echo "OK: K8s batch result protocol validation tests passed"
