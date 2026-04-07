#!/usr/bin/env bash
set -euo pipefail
#
# Deploy a conformance test runner pod inside the K8s cluster and run tests
# from there. Avoids port-forward instability with remote clusters.
#
# Usage:
#   ./scripts/in-cluster-runner.sh                    # Full run (build + deploy + run)
#   ./scripts/in-cluster-runner.sh --run-only         # Skip build, re-run in existing pod
#   ./scripts/in-cluster-runner.sh --core             # Only core features
#   ./scripts/in-cluster-runner.sh --cleanup          # Tear down runner pod + RBAC
#   ./scripts/in-cluster-runner.sh --force-recreate   # Recreate runner pod even if Ready
#   ./scripts/in-cluster-runner.sh --clean-slate      # Nuke all non-base resources, re-apply manifests, then run
#
# Split-profile running (required — Edgion cannot mix HTTPS Terminate and TLS
# Passthrough on port 443):
#
#   # Step 1: HTTP + gRPC
#   ./scripts/in-cluster-runner.sh --run-only --clean-slate \
#     --conformance-profiles "GATEWAY-HTTP,GATEWAY-GRPC" --timeout 30m
#
#   # Step 2: TLS only
#   ./scripts/in-cluster-runner.sh --run-only --clean-slate \
#     --conformance-profiles "GATEWAY-TLS" --timeout 20m
#
# Reports are saved to reports/ with profile-derived names.
#
# The script:
#   1. Cross-compiles the Go test binary (linux/amd64)
#   2. Creates a ServiceAccount + ClusterRole with wide permissions
#   3. Deploys an ubuntu:22.04 sleep pod
#   4. Copies the test binary into the pod
#   5. Runs the test inside the pod, streaming output

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CONFORMANCE_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
PROJECT_ROOT="$(cd "$CONFORMANCE_ROOT/../.." && pwd)"

NAMESPACE="edgion-system"
RUNNER_POD="conformance-runner"
RUNNER_SA="conformance-runner"
TEST_DIR="/opt/conformance"
TENANT_LABEL="system"
RUN_ONLY=false
CLEANUP_ONLY=false
FORCE_RECREATE=false
FORCE_COPY=false
CLEAN_SLATE=false
TIMEOUT="30m"
EXTRA_ARGS=()

# Conformance profiles and features — mirrors run.sh defaults
CONFORMANCE_PROFILES="GATEWAY-HTTP,GATEWAY-GRPC,GATEWAY-TLS"
GATEWAY_CLASS="edgion"
SUPPORTED_FEATURES=""

while [[ $# -gt 0 ]]; do
  case "$1" in
    --run-only)       RUN_ONLY=true;                shift ;;
    --cleanup)        CLEANUP_ONLY=true;            shift ;;
    --force-recreate) FORCE_RECREATE=true;          shift ;;
    --force-copy)     FORCE_COPY=true;              shift ;;
    --clean-slate)    CLEAN_SLATE=true;             shift ;;
    --timeout)        TIMEOUT="$2";                 shift 2 ;;
    --core)           SUPPORTED_FEATURES="Gateway,HTTPRoute,ReferenceGrant"; shift ;;
    --conformance-profiles) CONFORMANCE_PROFILES="$2"; shift 2 ;;
    --gateway-class)  GATEWAY_CLASS="$2";           shift 2 ;;
    --supported-features) SUPPORTED_FEATURES="$2";  shift 2 ;;
    --run-test)       EXTRA_ARGS+=("-run-test=$2"); shift 2 ;;
    -h|--help)
      echo "Usage: $0 [--run-only|--cleanup|--force-recreate|--force-copy|--clean-slate|--core|--timeout <dur>|--run-test <name>|--conformance-profiles <list>|--gateway-class <name>|--supported-features <list>]"
      exit 0 ;;
    *) echo "Unknown option: $1"; exit 1 ;;
  esac
done

is_runner_ready() {
  local phase ready
  phase=$(kubectl get pod "$RUNNER_POD" -n "$NAMESPACE" -o jsonpath='{.status.phase}' 2>/dev/null || true)
  ready=$(kubectl get pod "$RUNNER_POD" -n "$NAMESPACE" -o jsonpath='{.status.conditions[?(@.type=="Ready")].status}' 2>/dev/null || true)
  [[ "${phase}" == "Running" && "${ready}" == "True" ]]
}

require_gateway_service() {
  local gw_svc_ip
  gw_svc_ip=$(kubectl get svc edgion-gateway -n "$NAMESPACE" \
    -o jsonpath='{.spec.clusterIP}' 2>/dev/null || true)

  if [[ -n "${gw_svc_ip}" && "${gw_svc_ip}" != "None" ]]; then
    return
  fi

  echo ""
  echo "ERROR: Service edgion-gateway was not found in namespace ${NAMESPACE}, or it has no ClusterIP."
  echo "       Conformance requires the controller to publish this Service address into Gateway status.addresses."
  echo ""
  kubectl get svc -n "$NAMESPACE" edgion-gateway 2>/dev/null || true
  exit 1
}

cleanup_runner() {
  echo "=== Cleaning up conformance runner ==="
  kubectl delete pod "$RUNNER_POD" -n "$NAMESPACE" --ignore-not-found --wait=false
  kubectl delete clusterrolebinding "$RUNNER_SA" --ignore-not-found
  kubectl delete clusterrole "$RUNNER_SA" --ignore-not-found
  kubectl delete serviceaccount "$RUNNER_SA" -n "$NAMESPACE" --ignore-not-found
  echo "Done."
}

if [[ "$CLEANUP_ONLY" == "true" ]]; then
  cleanup_runner
  exit 0
fi

require_gateway_service

# ── Step 1: Cross-compile test binary ─────────────────────────────────────────
if [[ "$RUN_ONLY" != "true" ]]; then
  echo "=== [1/4] Cross-compiling conformance test binary (linux/amd64) ==="
  cd "$CONFORMANCE_ROOT"
  # Patch upstream conformance library (Go 1.24+ IsCA fix)
  source "$SCRIPT_DIR/patch-upstream.sh"
  GOOS=linux GOARCH=amd64 CGO_ENABLED=0 go test -c -o conformance-test.bin .
  echo "  Binary: $(ls -lh conformance-test.bin | awk '{print $5}')"

  pod_ready=false
  if is_runner_ready; then
    pod_ready=true
  fi

  if [[ "$FORCE_RECREATE" == "true" ]]; then
    echo ""
    echo "=== [2/4] Force recreate requested: deleting old runner pod ==="
    kubectl delete pod "$RUNNER_POD" -n "$NAMESPACE" --ignore-not-found --wait=false 2>/dev/null || true
    pod_ready=false
    sleep 2
  fi

  # ── Step 2: Create RBAC ──────────────────────────────────────────────────────
  echo ""
  echo "=== [2/4] Creating runner RBAC ==="
  kubectl apply -f - <<'YAML'
apiVersion: v1
kind: ServiceAccount
metadata:
  name: conformance-runner
  namespace: edgion-system
---
apiVersion: rbac.authorization.k8s.io/v1
kind: ClusterRole
metadata:
  name: conformance-runner
rules:
# The conformance framework needs broad access to create test fixtures
- apiGroups: [""]
  resources: ["*"]
  verbs: ["*"]
- apiGroups: ["apps"]
  resources: ["*"]
  verbs: ["*"]
- apiGroups: ["gateway.networking.k8s.io"]
  resources: ["*"]
  verbs: ["*"]
- apiGroups: ["edgion.io"]
  resources: ["*"]
  verbs: ["*"]
- apiGroups: ["rbac.authorization.k8s.io"]
  resources: ["*"]
  verbs: ["*"]
- apiGroups: ["coordination.k8s.io"]
  resources: ["*"]
  verbs: ["*"]
- apiGroups: ["admissionregistration.k8s.io"]
  resources: ["*"]
  verbs: ["*"]
- apiGroups: ["apiextensions.k8s.io"]
  resources: ["*"]
  verbs: ["*"]
- apiGroups: ["discovery.k8s.io"]
  resources: ["*"]
  verbs: ["*"]
---
apiVersion: rbac.authorization.k8s.io/v1
kind: ClusterRoleBinding
metadata:
  name: conformance-runner
roleRef:
  apiGroup: rbac.authorization.k8s.io
  kind: ClusterRole
  name: conformance-runner
subjects:
- kind: ServiceAccount
  name: conformance-runner
  namespace: edgion-system
YAML
  echo "  RBAC applied."

  # ── Step 3: Deploy or reuse runner pod ───────────────────────────────────────
  echo ""
  if [[ "$pod_ready" == "true" ]]; then
    echo "=== [3/4] Reusing existing ready runner pod ==="
  else
    echo "=== [3/4] Deploying runner pod ==="
    # Recreate non-ready pod to avoid being stuck with an unschedulable spec.
    kubectl delete pod "$RUNNER_POD" -n "$NAMESPACE" --ignore-not-found --wait=false 2>/dev/null || true
    sleep 2
    kubectl apply -f - <<YAML
apiVersion: v1
kind: Pod
metadata:
  name: ${RUNNER_POD}
  namespace: ${NAMESPACE}
  labels:
    app: conformance-runner
    meta.zoom.us/tenant: ${TENANT_LABEL}
spec:
  serviceAccount: ${RUNNER_SA}
  serviceAccountName: ${RUNNER_SA}
  containers:
  - name: runner
    image: ubuntu:22.04
    command: ["sleep", "infinity"]
    resources:
      requests:
        cpu: 100m
        memory: 128Mi
      limits:
        cpu: 2000m
        memory: 6Gi
    volumeMounts:
    - name: test-workspace
      mountPath: ${TEST_DIR}
  volumes:
  - name: test-workspace
    emptyDir: {}
YAML

    echo "  Waiting for runner pod to be ready..."
    kubectl wait --for=condition=Ready pod/"$RUNNER_POD" -n "$NAMESPACE" --timeout=120s
    echo "  Runner pod is ready."
  fi

  # ── Step 4: Copy test binary (conditional) ───────────────────────────────────
  # Default behavior:
  # - Reused ready pod: skip copy to speed up iterative runs.
  # - Fresh/recreated pod: copy required.
  # - --force-copy: always copy.
  should_copy=false
  if [[ "$FORCE_COPY" == "true" ]]; then
    should_copy=true
  elif [[ "$pod_ready" != "true" ]]; then
    should_copy=true
  fi

  echo ""
  if [[ "$should_copy" == "true" ]]; then
    echo "=== [4/4] Copying test binary to runner pod ==="
    kubectl cp "$CONFORMANCE_ROOT/conformance-test.bin" \
      "$NAMESPACE/$RUNNER_POD:$TEST_DIR/conformance-test.bin"
    kubectl exec -n "$NAMESPACE" "$RUNNER_POD" -- chmod +x "$TEST_DIR/conformance-test.bin"
    echo "  Binary copied."
  else
    echo "=== [4/4] Reusing existing binary in runner pod (skip copy) ==="
    if ! kubectl exec -n "$NAMESPACE" "$RUNNER_POD" -- test -x "$TEST_DIR/conformance-test.bin"; then
      echo "  Existing binary not found or not executable; copying now..."
      kubectl cp "$CONFORMANCE_ROOT/conformance-test.bin" \
        "$NAMESPACE/$RUNNER_POD:$TEST_DIR/conformance-test.bin"
      kubectl exec -n "$NAMESPACE" "$RUNNER_POD" -- chmod +x "$TEST_DIR/conformance-test.bin"
      echo "  Binary copied."
    fi
  fi
fi

# ── Profile detection helpers ────────────────────────────────────────────────
has_http_grpc_profile() {
  [[ "$CONFORMANCE_PROFILES" == *"GATEWAY-HTTP"* ]] || \
  [[ "$CONFORMANCE_PROFILES" == *"GATEWAY-GRPC"* ]]
}
has_tls_profile() {
  [[ "$CONFORMANCE_PROFILES" == *"GATEWAY-TLS"* ]]
}

# Derive report filename from profiles.
# GATEWAY-HTTP,GATEWAY-GRPC → gateway-http-grpc-conformance-report.yaml
# GATEWAY-TLS               → gateway-tls-conformance-report.yaml
derive_report_name() {
  local slug
  slug=$(echo "$CONFORMANCE_PROFILES" | tr ',' '\n' | \
    sed 's/^GATEWAY-//' | tr '[:upper:]' '[:lower:]' | \
    paste -sd '-' -)
  echo "gateway-${slug}-conformance-report.yaml"
}

REPORT_NAME="$(derive_report_name)"

# ── Pre-run cleanup ──────────────────────────────────────────────────────────
CONFORMANCE_NAMESPACES=(
  "gateway-conformance-infra"
  "gateway-conformance-app-backend"
  "gateway-conformance-web-backend"
  "gateway-conformance-mesh"
  "gateway-conformance-mesh-consumer"
)

# Build profile-specific base Gateway list.
# HTTP/GRPC manifests (conformance-gateway.yaml):
HTTP_GRPC_BASE_GATEWAYS=(
  "same-namespace"
  "same-namespace-with-https-listener"
  "all-namespaces"
  "backend-namespaces"
)
# TLS manifests (conformance-gateway-tls.yaml):
TLS_BASE_GATEWAYS=(
  "gateway-tlsroute"
  "gw-tlsroute-exact-hostname-x-1"
  "gw-tlsroute-more-specific-wc-hostname-x-2"
  "gw-tlsroute-less-specific-wc-hostname-x-3"
  "gw-tlsroute-empty-hostname-x-4"
  "gateway-tlsroute-terminate"
  "pre-bind-tls-mixed-8883"
)

build_base_gateways() {
  local -a gws=()
  if has_http_grpc_profile; then
    gws+=("${HTTP_GRPC_BASE_GATEWAYS[@]}")
  fi
  if has_tls_profile; then
    gws+=("${TLS_BASE_GATEWAYS[@]}")
  fi
  echo "${gws[@]}"
}

# Base ConfigMaps that the conformance framework or our manifests create.
BASE_CONFIGMAPS=(
  "coredns"
  "kube-root-ca.crt"
  "tls-checks-ca-certificate"
  "tls-validity-checks-ca-certificate"
  "tls-validity-checks-per-port-ca-certificate"
  "mismatch-ca-certificate"
)

# Helper: delete resources from a namespace, optionally keeping a base list.
# Usage: delete_non_base <kind> <namespace> [base1 base2 ...]
delete_non_base() {
  local kind="$1" ns="$2"; shift 2
  local -a bases=("$@")
  local all_names name is_base stale="" count

  all_names=$(kubectl get "$kind" -n "$ns" \
    -o jsonpath='{range .items[*]}{.metadata.name}{"\n"}{end}' 2>/dev/null || true)
  [[ -z "$all_names" ]] && return

  for name in $all_names; do
    is_base=false
    for b in "${bases[@]+"${bases[@]}"}"; do
      [[ "$name" == "$b" ]] && { is_base=true; break; }
    done
    [[ "$is_base" == "false" ]] && stale="${stale:+$stale }$name"
  done

  if [[ -n "$stale" ]]; then
    count=$(echo "$stale" | wc -w | tr -d ' ')
    echo "  ${ns}: deleting ${count} ${kind}(s): $stale"
    for name in $stale; do
      kubectl delete "$kind" "$name" -n "$ns" --ignore-not-found 2>/dev/null || true
    done
  fi
}

restart_process() {
  local label="$1" binary="$2"
  local pod
  pod=$(kubectl get pods -n "$NAMESPACE" -l "app=$label" \
    -o jsonpath='{.items[0].metadata.name}' 2>/dev/null || true)
  if [[ -z "$pod" ]]; then
    echo "    WARNING: no $label pod found, skipping restart"
    return
  fi
  echo "    Restarting $label (pod: $pod)..."
  kubectl exec -n "$NAMESPACE" "$pod" -- pkill -f "$binary" 2>/dev/null || true
  local ready=false
  for _ in $(seq 1 20); do
    sleep 3
    if kubectl exec -n "$NAMESPACE" "$pod" -- pgrep -f "$binary" >/dev/null 2>&1; then
      ready=true
      break
    fi
  done
  if [[ "$ready" == "true" ]]; then
    echo "    $label restarted."
  else
    echo "    WARNING: $label may not have restarted — check pod logs."
  fi
}

wait_controller_gateway_sync() {
  local ctrl_pod gw_count timeout=60 elapsed=0
  ctrl_pod=$(kubectl get pods -n "$NAMESPACE" -l "app=edgion-controller" \
    -o jsonpath='{.items[0].metadata.name}' 2>/dev/null || true)
  [[ -z "$ctrl_pod" ]] && { echo "    WARNING: no controller pod, skipping sync wait"; return; }

  gw_count=$(kubectl get gateway -n gateway-conformance-infra --no-headers 2>/dev/null | wc -l | tr -d ' ')
  echo "    Waiting for controller to sync $gw_count Gateway(s) to gateway..."

  local gw_pod
  gw_pod=$(kubectl get pods -n "$NAMESPACE" -l "app=edgion-gateway" \
    -o jsonpath='{.items[0].metadata.name}' 2>/dev/null || true)
  [[ -z "$gw_pod" ]] && { echo "    WARNING: no gateway pod, skipping sync wait"; return; }

  while (( elapsed < timeout )); do
    local stored
    stored=$(kubectl exec -n "$NAMESPACE" "$gw_pod" -- \
      grep -c '"Gateway stored"' /usr/local/edgion/logs/edgion-gateway.log 2>/dev/null || echo "0")
    if (( stored >= gw_count )); then
      echo "    Gateway received $stored Gateway sync event(s) (need $gw_count). Sync complete."
      return
    fi
    sleep 3
    elapsed=$((elapsed + 3))
  done
  echo "    WARNING: timed out waiting for controller→gateway sync ($elapsed s). Continuing anyway."
}

restart_controller_and_gateway() {
  echo "  Restarting controller + gateway to pick up new manifest state..."

  # 1. Restart controller first so it re-lists all K8s resources.
  restart_process "edgion-controller" "edgion-controller"

  # 2. Wait for controller to complete its initial K8s list+watch cycle.
  #    The controller needs time to re-list all Gateways from K8s and populate
  #    its gRPC cache. If the gateway restarts too early, it may pull stale data.
  echo "    Waiting 20s for controller initial sync..."
  sleep 20

  # 3. Restart gateway so it rebinds physical listeners from controller cache.
  restart_process "edgion-gateway" "edgion-gateway"

  # 4. Wait for the gateway to receive all Gateway objects from controller.
  wait_controller_gateway_sync

  echo "  Waiting 10s for route sync to settle..."
  sleep 10
}

if [[ "$CLEAN_SLATE" == "true" ]]; then
  # ── Full reset: nuke all non-base resources across all conformance namespaces.
  echo ""
  echo "=== Clean-slate: removing ALL non-base test resources ==="
  echo "  Active profiles: ${CONFORMANCE_PROFILES}"

  # Build profile-appropriate base Gateway list.
  read -ra BASE_GATEWAYS <<< "$(build_base_gateways)"

  for ns in "${CONFORMANCE_NAMESPACES[@]}"; do
    kubectl get ns "$ns" >/dev/null 2>&1 || continue

    # Routes & policies — none are base, delete all.
    for kind in httproute grpcroute tlsroute tcproute udproute backendtlspolicy referencegrant; do
      items=$(kubectl get "$kind" -n "$ns" -o name 2>/dev/null || true)
      if [[ -n "$items" ]]; then
        count=$(echo "$items" | wc -l | tr -d ' ')
        echo "  ${ns}: deleting ${count} ${kind}(s)"
        echo "$items" | xargs kubectl delete -n "$ns" --ignore-not-found 2>/dev/null || true
      fi
    done

    # Gateways — keep only those needed for the active profiles.
    delete_non_base gateway "$ns" "${BASE_GATEWAYS[@]}"

    # ConfigMaps — keep base ones.
    delete_non_base configmap "$ns" "${BASE_CONFIGMAPS[@]}"

    # Secrets: keep base programmatic certs (re-creating them causes TLS reload
    # latency in the data plane) and registry credentials.
    delete_non_base secret "$ns" \
      "regcred-jfrog" \
      "certificate" \
      "tls-checks-certificate" \
      "tls-checks-client-certificate" \
      "tls-passthrough-checks-certificate" \
      "tls-terminate-checks-certificate" \
      "tls-validity-checks-certificate" \
      "tls-validity-checks-client-certificate" \
      "tls-validity-checks-per-port-client-certificate"

    # Services & Deployments created by tests (not base backends).
    for kind in service deployment; do
      delete_non_base "$kind" "$ns" \
        "infra-backend-v1" "infra-backend-v2" "infra-backend-v3" \
        "grpc-infra-backend-v1" "grpc-infra-backend-v2" "grpc-infra-backend-v3" \
        "tls-backend" "tls-backend-2" "tcp-backend" "coredns"
    done
  done

  # Re-apply only the manifests needed for the active profiles.
  echo ""
  echo "  Re-applying profile-specific base manifests..."
  if has_http_grpc_profile; then
    echo "    → conformance-gateway.yaml (HTTP/GRPC)"
    kubectl apply -f "$CONFORMANCE_ROOT/manifests/conformance-gateway.yaml"
  fi
  if has_tls_profile; then
    echo "    → conformance-gateway-tls.yaml (TLS)"
    kubectl apply -f "$CONFORMANCE_ROOT/manifests/conformance-gateway-tls.yaml"
  fi

  # TLS backend pods mount certificate Secrets as volumes and load them once
  # at startup.  When clean-slate deletes and recreates Secrets (new key
  # pairs), running pods still serve the old certificates, causing x509
  # verification failures.  Rolling restart forces them to pick up the new
  # certs from the freshly-created Secrets.
  if has_tls_profile; then
    echo ""
    echo "  Rolling restart TLS backend pods to pick up fresh certificates..."
    TLS_BACKEND_DEPLOYMENTS=(
      "gateway-conformance-infra/tls-backend"
      "gateway-conformance-infra/tls-backend-2"
      "gateway-conformance-infra/tcp-backend"
      "gateway-conformance-app-backend/tls-backend"
    )
    for nsdep in "${TLS_BACKEND_DEPLOYMENTS[@]}"; do
      ns="${nsdep%%/*}"
      dep="${nsdep##*/}"
      if kubectl get deployment "$dep" -n "$ns" >/dev/null 2>&1; then
        kubectl rollout restart deployment "$dep" -n "$ns"
      fi
    done
    for nsdep in "${TLS_BACKEND_DEPLOYMENTS[@]}"; do
      ns="${nsdep%%/*}"
      dep="${nsdep##*/}"
      if kubectl get deployment "$dep" -n "$ns" >/dev/null 2>&1; then
        kubectl rollout status deployment "$dep" -n "$ns" --timeout=120s
      fi
    done
    echo "  TLS backends restarted."
  fi

  # Restart controller (to re-sync Gateway state from K8s) then gateway
  # (to rebind physical listeners with the correct set of Gateways).
  restart_controller_and_gateway

  echo "  Clean-slate done."

else
  # ── Lightweight default: only delete known stale ConfigMaps.
  echo ""
  echo "=== Pre-run cleanup: removing stale ConfigMaps ==="
  delete_non_base configmap "gateway-conformance-infra" "${BASE_CONFIGMAPS[@]}"
  echo "  Pre-run cleanup done."
fi

# ── Run tests inside the pod ─────────────────────────────────────────────────
echo ""
echo "==========================================="
echo "  Running conformance tests in-cluster"
echo "==========================================="
echo "  GatewayClass      : ${GATEWAY_CLASS}"
echo "  Profiles           : ${CONFORMANCE_PROFILES}"
echo "  Features           : ${SUPPORTED_FEATURES:-<from test code>}"
echo "  Timeout            : ${TIMEOUT}"
echo "  Report             : reports/${REPORT_NAME}"
echo ""

TEST_ARGS=(
  "-test.v"
  "-test.timeout=${TIMEOUT}"
  "-test.count=1"
  "-test.run=TestConformance"
  "-gateway-class=${GATEWAY_CLASS}"
  "-cleanup-base-resources=false"
  "-debug=true"
  "-allow-crds-mismatch"
  "-organization=Edgion"
  "-project=Edgion"
  "-url=https://github.com/edgion/edgion"
  "-version=dev"
  "-contact=@edgion"
  "-report-output=${TEST_DIR}/${REPORT_NAME}"
  "-conformance-profiles=${CONFORMANCE_PROFILES}"
)

if [[ -n "${SUPPORTED_FEATURES}" ]]; then
  TEST_ARGS+=("-supported-features=${SUPPORTED_FEATURES}")
fi

for arg in "${EXTRA_ARGS[@]+"${EXTRA_ARGS[@]}"}"; do
  TEST_ARGS+=("$arg")
done

echo "Command: ${TEST_DIR}/conformance-test.bin ${TEST_ARGS[*]}"
echo ""

# For TLS-only runs, override base manifests to exclude HTTP Gateways that
# conflict with TLS Passthrough on port 443.
ENV_ARGS=()
if has_tls_profile && ! has_http_grpc_profile; then
  echo "  [split-profile] TLS-only: using base-manifests-tls-only.yaml"
  # Copy the TLS-only base manifest into the runner pod.
  kubectl cp "$CONFORMANCE_ROOT/manifests/base-manifests-tls-only.yaml" \
    "$NAMESPACE/$RUNNER_POD:$TEST_DIR/base-manifests-tls-only.yaml"
  ENV_ARGS+=("EDGION_BASE_MANIFESTS=$TEST_DIR/base-manifests-tls-only.yaml")
fi

# Run inside the pod, streaming stdout/stderr back.
# Temporarily disable errexit so we can capture the exit code even on failure.
set +e
kubectl exec -n "$NAMESPACE" "$RUNNER_POD" -- \
  env "${ENV_ARGS[@]+"${ENV_ARGS[@]}"}" \
  "$TEST_DIR/conformance-test.bin" "${TEST_ARGS[@]}" 2>&1
RC=$?
set -e

echo ""
if [[ "$RC" -eq 0 ]]; then
  echo "=== Conformance Tests PASSED ==="
else
  echo "=== Conformance Tests FAILED (exit code: $RC) ==="
fi

# Copy report back to reports/ directory.
mkdir -p "$CONFORMANCE_ROOT/reports"
kubectl cp "$NAMESPACE/$RUNNER_POD:$TEST_DIR/${REPORT_NAME}" \
  "$CONFORMANCE_ROOT/reports/${REPORT_NAME}" 2>/dev/null || true

if [[ -f "$CONFORMANCE_ROOT/reports/${REPORT_NAME}" ]]; then
  echo "Report saved to: reports/${REPORT_NAME}"
fi

exit "$RC"
