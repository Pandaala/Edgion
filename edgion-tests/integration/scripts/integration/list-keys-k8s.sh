#!/usr/bin/env bash
# =============================================================================
# list-keys-k8s.sh
#
# Verifies new /api/v1/keys/* endpoints in K8s mode:
#   - Metadata-only response (no spec, no status on data items)
#   - Real pagination (3-page traversal of 25 HTTPRoutes, names unique)
#   - 410 Gone on stale continue token (accepts 400 since the kube apiserver
#     may reject a syntactically-garbage token before pagination logic runs;
#     the 410 mapping itself is covered by the Rust unit test in
#     kubernetes/storage.rs::tests::map_kube_error_410_to_stale_pagination)
#   - Regression: legacy /api/v1/namespaced/HTTPRoute/{ns} still returns
#     full content (data[0].spec present)
#
# Requires:
#   - kubectl pointing at a live K8s cluster with Edgion's CRDs installed
#   - edgion-controller running in K8s mode and reachable at ADMIN_URL
#   - ADMIN_TOKEN env var with a valid bearer token (or unauthenticated admin)
#   - jq
#
# Default ADMIN_URL: http://127.0.0.1:12101
# =============================================================================
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=../utils/shell_compat.sh
source "${SCRIPT_DIR}/../utils/shell_compat.sh"

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m'

log_info()    { echo -e "${BLUE}[INFO]${NC} $1"; }
log_success() { echo -e "${GREEN}[OK]${NC}   $1"; }
log_error()   { echo -e "${RED}[ERR]${NC}  $1"; }
log_warn()    { echo -e "${YELLOW}[WARN]${NC} $1"; }
log_section() { echo; echo -e "${BLUE}=== $1 ===${NC}"; }

# --- Prereqs ---
command -v jq >/dev/null      || { log_warn "jq not installed; SKIP"; exit 0; }
command -v kubectl >/dev/null || { log_warn "kubectl not installed; SKIP"; exit 0; }

if ! kubectl version --client > /dev/null 2>&1; then
    log_warn "kubectl not usable; SKIP"
    exit 0
fi
if ! kubectl get crd httproutes.gateway.networking.k8s.io > /dev/null 2>&1; then
    log_warn "Gateway API HTTPRoute CRD not installed on target cluster; SKIP"
    exit 0
fi

ADMIN_URL="${ADMIN_URL:-http://127.0.0.1:12101}"
# /health lives on the dedicated probe listener, not the admin API.
PROBE_URL="${PROBE_URL:-http://127.0.0.1:12100}"
if ! curl -sf --max-time 3 "${PROBE_URL}/health" > /dev/null 2>&1; then
    log_warn "edgion-controller probe endpoint not reachable at $PROBE_URL; SKIP"
    exit 0
fi

curl_admin() {
    if [[ -n "${ADMIN_TOKEN:-}" ]]; then
        curl -s -H "Authorization: Bearer ${ADMIN_TOKEN}" "$@"
    else
        curl -s "$@"
    fi
}

# --- Setup ---
NAMESPACE="list-keys-test-$(date +%s)"
log_section "Setup namespace=$NAMESPACE"
kubectl create namespace "$NAMESPACE" > /dev/null

cleanup() {
    log_section "Cleanup"
    kubectl delete namespace "$NAMESPACE" --wait=false > /dev/null 2>&1 || true
}
trap cleanup EXIT

# Apply 25 minimal HTTPRoutes
for i in $(seq 1 25); do
    cat <<EOF | kubectl apply -f - > /dev/null
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: route-${i}
  namespace: ${NAMESPACE}
spec:
  rules:
    - matches:
        - path:
            type: PathPrefix
            value: /r${i}
EOF
done
log_info "Applied 25 HTTPRoutes to $NAMESPACE"

# Wait for controller to sync
sleep 3

# --- Probe authorization once ---
PROBE_STATUS=$(curl_admin -o /dev/null -w "%{http_code}" \
    "${ADMIN_URL}/api/v1/keys/namespaced/HTTPRoute/${NAMESPACE}?limit=1")
case "$PROBE_STATUS" in
    200) : ;;
    401|403)
        log_warn "Admin endpoint requires auth (got $PROBE_STATUS); export ADMIN_TOKEN; SKIP"
        exit 0
        ;;
    *)
        log_error "Probe failed: HTTP $PROBE_STATUS"
        exit 1
        ;;
esac

# --- Page 1 ---
log_section "Page 1 (limit=10)"
PAGE1=$(curl_admin "${ADMIN_URL}/api/v1/keys/namespaced/HTTPRoute/${NAMESPACE}?limit=10")
echo "$PAGE1" | jq -e '.data | length == 10' >/dev/null \
    || { log_error "Page 1 length != 10"; echo "$PAGE1" | jq .; exit 1; }
# Structural projection check: each item must have ONLY the ResourceKey-shaped
# keys (apiVersion / kind / metadata). `.spec == null` alone would be vacuous
# because ResourceKey is a typed Rust struct without that field.
echo "$PAGE1" | jq -e '.data | all((keys | sort) == ["apiVersion","kind","metadata"])' >/dev/null \
    || { log_error "Page 1 item key-set is not exactly {apiVersion, kind, metadata}; projection let extra fields through"; echo "$PAGE1" | jq '.data[0]'; exit 1; }
echo "$PAGE1" | jq -e '.data | all(.metadata.name != null and .metadata.name != "")' >/dev/null \
    || { log_error "Page 1 some item missing metadata.name"; exit 1; }
echo "$PAGE1" | jq -e '.continue_token != null and (.continue_token | length > 0)' >/dev/null \
    || { log_error "Page 1 continue_token missing"; exit 1; }
CONT1=$(echo "$PAGE1" | jq -r '.continue_token')
NAMES1=$(echo "$PAGE1" | jq -r '.data[].metadata.name')
log_success "Page 1: 10 items, no spec/status, continue token present"

# --- Page 2 ---
log_section "Page 2"
# Use -G + --data-urlencode so a base64 continue token containing '/', '+', or
# '=' survives transit unchanged.
PAGE2=$(curl_admin -G \
    --data-urlencode "limit=10" \
    --data-urlencode "continue=${CONT1}" \
    "${ADMIN_URL}/api/v1/keys/namespaced/HTTPRoute/${NAMESPACE}")
echo "$PAGE2" | jq -e '.data | length == 10' >/dev/null \
    || { log_error "Page 2 length != 10"; exit 1; }
CONT2=$(echo "$PAGE2" | jq -r '.continue_token')
NAMES2=$(echo "$PAGE2" | jq -r '.data[].metadata.name')
log_success "Page 2: 10 items"

# --- Page 3 ---
log_section "Page 3"
PAGE3=$(curl_admin -G \
    --data-urlencode "limit=10" \
    --data-urlencode "continue=${CONT2}" \
    "${ADMIN_URL}/api/v1/keys/namespaced/HTTPRoute/${NAMESPACE}")
echo "$PAGE3" | jq -e '.data | length == 5' >/dev/null \
    || { log_error "Page 3 length != 5"; exit 1; }
echo "$PAGE3" | jq -e '.continue_token == null or .continue_token == ""' >/dev/null \
    || { log_error "Page 3 continue_token should be empty"; exit 1; }
NAMES3=$(echo "$PAGE3" | jq -r '.data[].metadata.name')
log_success "Page 3: 5 items, last page"

# --- Uniqueness ---
log_section "Uniqueness check"
# grep -c reports status 1 for zero matches even though it prints 0. Preserve
# that value so the assertion below reports the real failure under set -e.
ALL=$(printf '%s\n%s\n%s\n' "$NAMES1" "$NAMES2" "$NAMES3" | edgion_count_nonempty_lines)
UNIQ=$(printf '%s\n%s\n%s\n' "$NAMES1" "$NAMES2" "$NAMES3" | sort -u | edgion_count_nonempty_lines)
[[ "$ALL" == "25" && "$UNIQ" == "25" ]] \
    || { log_error "Expected 25 unique names, got $UNIQ unique of $ALL total"; exit 1; }
log_success "25 unique names across all pages"

# --- 410 stale-token path ---
log_section "Stale continue token"
STALE_STATUS=$(curl_admin -G -o /dev/null -w "%{http_code}" \
    --data-urlencode "limit=10" \
    --data-urlencode "continue=ZmFrZS1leHBpcmVkLXRva2Vu" \
    "${ADMIN_URL}/api/v1/keys/namespaced/HTTPRoute/${NAMESPACE}")
if [[ "$STALE_STATUS" == "410" || "$STALE_STATUS" == "400" ]]; then
    log_success "Stale token returned $STALE_STATUS (410 or 400 acceptable)"
else
    log_error "Stale token expected 410 or 400, got $STALE_STATUS"
    exit 1
fi

# --- Regression: legacy endpoint ---
log_section "Regression: legacy /api/v1/namespaced/HTTPRoute/{ns}"
LEGACY=$(curl_admin "${ADMIN_URL}/api/v1/namespaced/HTTPRoute/${NAMESPACE}")
LEGACY_LEN=$(echo "$LEGACY" | jq '.data | length' 2>/dev/null || echo 0)
[[ "$LEGACY_LEN" -ge 25 ]] \
    || { log_error "Legacy path returned only $LEGACY_LEN items (expected >=25, full list)"; exit 1; }
echo "$LEGACY" | jq -e '.data | any(.spec != null)' >/dev/null \
    || { log_error "Legacy path returned no spec on any item — regression!"; exit 1; }
log_success "Legacy path returned $LEGACY_LEN items with spec preserved"

log_section "PASS: list-keys-k8s.sh"
