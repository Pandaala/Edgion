#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
VALIDATOR="${SCRIPT_DIR}/../validate_fixture_ownership.sh"
TEMP_DIR="$(mktemp -d /tmp/edgion-fixture-ownership-test.XXXXXX)"

cleanup() {
  rm -rf -- "${TEMP_DIR}"
}
trap cleanup EXIT INT TERM

fail() {
  echo "ERROR: $*" >&2
  exit 1
}

assert_contains() {
  local file="$1"
  local expected="$2"
  if ! grep -Fq -- "${expected}" "${file}"; then
    echo "ERROR: expected output to contain: ${expected}" >&2
    cat "${file}" >&2
    exit 1
  fi
}

mkdir -p "${TEMP_DIR}/multi"
cat > "${TEMP_DIR}/multi/resources.yaml" <<'EOF'
apiVersion: v1
kind: ConfigMap
metadata:
  name: first
  namespace: test
data:
  value: one
---
apiVersion: v1
kind: Service
metadata:
  name: backend
  namespace: test
spec:
  selector:
    app: backend
EOF

"${VALIDATOR}" "${TEMP_DIR}/multi" >"${TEMP_DIR}/multi.log" 2>&1 \
  || fail "validator rejected unique resources in a multi-document YAML file"
assert_contains "${TEMP_DIR}/multi.log" "validation passed: every effective initial object has one fixture owner"

mkdir -p "${TEMP_DIR}/duplicate-key"
cat > "${TEMP_DIR}/duplicate-key/missing-separator.yaml" <<'EOF'
apiVersion: v1
kind: Service
metadata:
  name: backend
  namespace: test
spec:
  selector:
    app: backend
apiVersion: v1
kind: ConfigMap
metadata:
  name: settings
  namespace: test
EOF

if "${VALIDATOR}" "${TEMP_DIR}/duplicate-key" >"${TEMP_DIR}/duplicate-key.log" 2>&1; then
  fail "validator accepted duplicate YAML mapping keys caused by a missing document separator"
fi
assert_contains "${TEMP_DIR}/duplicate-key.log" "found duplicate mapping key 'apiVersion'"

mkdir -p "${TEMP_DIR}/identical/a" "${TEMP_DIR}/identical/b"
cat > "${TEMP_DIR}/identical/a/first.yaml" <<'EOF'
apiVersion: v1
kind: ConfigMap
metadata:
  namespace: test
  name: shared
data:
  alpha: one
  beta: two
EOF
cat > "${TEMP_DIR}/identical/b/second.yaml" <<'EOF'
kind: ConfigMap
apiVersion: v1
data:
  beta: two
  alpha: one
metadata:
  name: shared
  namespace: test
EOF

"${VALIDATOR}" "${TEMP_DIR}/identical" >"${TEMP_DIR}/identical.log" 2>&1 \
  || fail "validator rejected duplicate-identical resources by default"
assert_contains "${TEMP_DIR}/identical.log" "[duplicate-identical] apiVersion=v1 kind=ConfigMap namespace=test name=shared"
assert_contains "${TEMP_DIR}/identical.log" "a/first.yaml#doc=1"
assert_contains "${TEMP_DIR}/identical.log" "b/second.yaml#doc=1"

if "${VALIDATOR}" --fail-on-identical "${TEMP_DIR}/identical" >"${TEMP_DIR}/identical-strict.log" 2>&1; then
  fail "--fail-on-identical accepted duplicate-identical resources"
fi
assert_contains "${TEMP_DIR}/identical-strict.log" "--fail-on-identical is set"

mkdir -p "${TEMP_DIR}/conflicting"
cat > "${TEMP_DIR}/conflicting/multi.yaml" <<'EOF'
apiVersion: v1
kind: ConfigMap
metadata:
  name: shared
  namespace: test
data:
  value: first
---
apiVersion: v1
kind: ConfigMap
metadata:
  name: unrelated
  namespace: test
EOF
cat > "${TEMP_DIR}/conflicting/other.yaml" <<'EOF'
apiVersion: v1
kind: ConfigMap
metadata:
  name: shared
  namespace: test
data:
  value: second
EOF

if "${VALIDATOR}" "${TEMP_DIR}/conflicting" >"${TEMP_DIR}/conflicting.log" 2>&1; then
  fail "validator accepted conflicting resource owners"
fi
assert_contains "${TEMP_DIR}/conflicting.log" "[duplicate-conflicting] apiVersion=v1 kind=ConfigMap namespace=test name=shared"
assert_contains "${TEMP_DIR}/conflicting.log" "multi.yaml#doc=1"
assert_contains "${TEMP_DIR}/conflicting.log" "other.yaml#doc=1"

EXCLUDED_ROOT="${TEMP_DIR}/excluded"
mkdir -p \
  "${EXCLUDED_ROOT}/Gateway/DynamicTest/initial" \
  "${EXCLUDED_ROOT}/Gateway/DynamicTest/updates" \
  "${EXCLUDED_ROOT}/EdgionPlugins/BasicAuth/DynamicTest/initial_grouped" \
  "${EXCLUDED_ROOT}/EdgionPlugins/BasicAuth/DynamicTest/updates_grouped" \
  "${EXCLUDED_ROOT}/EdgionPlugins/RateLimit/DynamicTest/initial" \
  "${EXCLUDED_ROOT}/EdgionPlugins/RateLimit/DynamicTest/updates" \
  "${EXCLUDED_ROOT}/EdgionPlugins/RateLimit/DynamicTest/staged/delete_grouped" \
  "${EXCLUDED_ROOT}/Gateway/PortConflict" \
  "${EXCLUDED_ROOT}/base" \
  "${EXCLUDED_ROOT}/active"

cat > "${EXCLUDED_ROOT}/Gateway/DynamicTest/initial/Gateway.yaml" <<'EOF'
apiVersion: gateway.networking.k8s.io/v1
kind: Gateway
metadata:
  name: dynamic
  namespace: test
spec:
  gatewayClassName: test
EOF
cat > "${EXCLUDED_ROOT}/Gateway/DynamicTest/updates/Gateway.yaml" <<'EOF'
apiVersion: gateway.networking.k8s.io/v1
kind: Gateway
metadata:
  name: dynamic
  namespace: test
spec:
  gatewayClassName: conflicting-update
EOF
cat > "${EXCLUDED_ROOT}/EdgionPlugins/BasicAuth/DynamicTest/initial_grouped/ConfigMap.yaml" <<'EOF'
apiVersion: v1
kind: ConfigMap
metadata:
  name: basic-auth-dynamic
  namespace: test
data:
  phase: initial-grouped
EOF
cat > "${EXCLUDED_ROOT}/EdgionPlugins/BasicAuth/DynamicTest/updates_grouped/ConfigMap.yaml" <<'EOF'
apiVersion: v1
kind: ConfigMap
metadata:
  name: basic-auth-dynamic
  namespace: test
data:
  phase: updates-grouped
EOF
cat > "${EXCLUDED_ROOT}/EdgionPlugins/RateLimit/DynamicTest/initial/ConfigMap.yaml" <<'EOF'
apiVersion: v1
kind: ConfigMap
metadata:
  name: rate-limit-dynamic
  namespace: test
data:
  phase: initial
EOF
cat > "${EXCLUDED_ROOT}/EdgionPlugins/RateLimit/DynamicTest/updates/ConfigMap.yaml" <<'EOF'
apiVersion: v1
kind: ConfigMap
metadata:
  name: rate-limit-dynamic
  namespace: test
data:
  phase: updates
EOF
cat > "${EXCLUDED_ROOT}/EdgionPlugins/RateLimit/DynamicTest/staged/delete_grouped/ConfigMap.yaml" <<'EOF'
apiVersion: v1
kind: ConfigMap
metadata:
  name: rate-limit-dynamic
  namespace: test
data:
  phase: delete-grouped
EOF
cat > "${EXCLUDED_ROOT}/active/edge-tls.yaml" <<'EOF'
apiVersion: v1
kind: Secret
metadata:
  name: edge-tls
  namespace: edgion-test
stringData:
  source: generated
EOF
cat > "${EXCLUDED_ROOT}/base/Secret_edgion-test_edge-tls.yaml" <<'EOF'
apiVersion: v1
kind: Secret
metadata:
  name: edge-tls
  namespace: edgion-test
stringData:
  source: template
EOF
cat > "${EXCLUDED_ROOT}/active/port-conflict.yaml" <<'EOF'
apiVersion: gateway.networking.k8s.io/v1
kind: Gateway
metadata:
  name: port-conflict-internal
  namespace: test
spec:
  gatewayClassName: valid-owner
EOF
cat > "${EXCLUDED_ROOT}/Gateway/PortConflict/Gateway_internal_conflict.yaml" <<'EOF'
apiVersion: gateway.networking.k8s.io/v1
kind: Gateway
metadata:
  name: port-conflict-internal
  namespace: test
spec:
  gatewayClassName: invalid-admission-fixture
EOF
cat > "${EXCLUDED_ROOT}/active/namespace.yaml" <<'EOF'
apiVersion: v1
kind: Namespace
metadata:
  name: bootstrap-test
EOF
cat > "${EXCLUDED_ROOT}/00-namespace.yaml" <<'EOF'
apiVersion: v1
kind: Namespace
metadata:
  name: bootstrap-test
  labels:
    source: bootstrap
EOF

"${VALIDATOR}" "${EXCLUDED_ROOT}" >"${TEMP_DIR}/excluded.log" 2>&1 \
  || fail "validator did not exclude dynamic updates, runtime Secret templates, bootstrap, or invalid admission fixtures"
assert_contains "${TEMP_DIR}/excluded.log" "scanned 6 effective YAML file(s); excluded 7 file(s)"

if "${VALIDATOR}" --include-dynamic-updates "${EXCLUDED_ROOT}" >"${TEMP_DIR}/include-dynamic.log" 2>&1; then
  fail "--include-dynamic-updates did not expose the conflicting update fixture"
fi
assert_contains "${TEMP_DIR}/include-dynamic.log" "Gateway/DynamicTest/updates/Gateway.yaml#doc=1"
assert_contains "${TEMP_DIR}/include-dynamic.log" "EdgionPlugins/BasicAuth/DynamicTest/updates_grouped/ConfigMap.yaml#doc=1"
assert_contains "${TEMP_DIR}/include-dynamic.log" "EdgionPlugins/RateLimit/DynamicTest/updates/ConfigMap.yaml#doc=1"
assert_contains "${TEMP_DIR}/include-dynamic.log" "EdgionPlugins/RateLimit/DynamicTest/staged/delete_grouped/ConfigMap.yaml#doc=1"

if "${VALIDATOR}" --include-bootstrap "${EXCLUDED_ROOT}" >"${TEMP_DIR}/include-bootstrap.log" 2>&1; then
  fail "--include-bootstrap did not expose the conflicting bootstrap fixture"
fi
assert_contains "${TEMP_DIR}/include-bootstrap.log" "00-namespace.yaml#doc=1"

echo "OK: fixture ownership validator regression tests passed"
