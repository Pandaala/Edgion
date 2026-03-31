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
RUN_ONLY=false
CLEANUP_ONLY=false
TIMEOUT="30m"
EXTRA_ARGS=()

# Conformance profiles and features — mirrors run.sh defaults
CONFORMANCE_PROFILES="GATEWAY-HTTP,GATEWAY-GRPC"
GATEWAY_CLASS="edgion"
SUPPORTED_FEATURES=""

while [[ $# -gt 0 ]]; do
  case "$1" in
    --run-only)       RUN_ONLY=true;                shift ;;
    --cleanup)        CLEANUP_ONLY=true;            shift ;;
    --timeout)        TIMEOUT="$2";                 shift 2 ;;
    --core)           SUPPORTED_FEATURES="Gateway,HTTPRoute,ReferenceGrant"; shift ;;
    --conformance-profiles) CONFORMANCE_PROFILES="$2"; shift 2 ;;
    --gateway-class)  GATEWAY_CLASS="$2";           shift 2 ;;
    --supported-features) SUPPORTED_FEATURES="$2";  shift 2 ;;
    --run-test)       EXTRA_ARGS+=("-run-test=$2"); shift 2 ;;
    -h|--help)
      echo "Usage: $0 [--run-only|--cleanup|--core|--timeout <dur>|--run-test <name>]"
      exit 0 ;;
    *) echo "Unknown option: $1"; exit 1 ;;
  esac
done

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

# ── Step 1: Cross-compile test binary ─────────────────────────────────────────
if [[ "$RUN_ONLY" != "true" ]]; then
  echo "=== [1/4] Cross-compiling conformance test binary (linux/amd64) ==="
  cd "$CONFORMANCE_ROOT"
  GOOS=linux GOARCH=amd64 CGO_ENABLED=0 go test -c -o conformance-test.bin .
  echo "  Binary: $(ls -lh conformance-test.bin | awk '{print $5}')"

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

  # ── Step 3: Deploy runner pod ────────────────────────────────────────────────
  echo ""
  echo "=== [3/4] Deploying runner pod ==="
  # Delete old pod if exists
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
spec:
  serviceAccountName: ${RUNNER_SA}
  containers:
  - name: runner
    image: ubuntu:22.04
    command: ["sleep", "infinity"]
    resources:
      requests:
        cpu: 500m
        memory: 512Mi
      limits:
        cpu: 2000m
        memory: 2Gi
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

  # ── Step 4: Copy test binary ─────────────────────────────────────────────────
  echo ""
  echo "=== [4/4] Copying test binary to runner pod ==="
  kubectl cp "$CONFORMANCE_ROOT/conformance-test.bin" \
    "$NAMESPACE/$RUNNER_POD:$TEST_DIR/conformance-test.bin"
  kubectl exec -n "$NAMESPACE" "$RUNNER_POD" -- chmod +x "$TEST_DIR/conformance-test.bin"
  echo "  Binary copied."
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
  "-report-output=${TEST_DIR}/conformance-report.yaml"
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

# Run inside the pod, streaming stdout/stderr back
kubectl exec -n "$NAMESPACE" "$RUNNER_POD" -- \
  "$TEST_DIR/conformance-test.bin" "${TEST_ARGS[@]}" 2>&1
RC=$?

echo ""
if [[ "$RC" -eq 0 ]]; then
  echo "=== Conformance Tests PASSED ==="
else
  echo "=== Conformance Tests FAILED (exit code: $RC) ==="
fi

# Copy report back
kubectl cp "$NAMESPACE/$RUNNER_POD:$TEST_DIR/conformance-report.yaml" \
  "$CONFORMANCE_ROOT/conformance-report.yaml" 2>/dev/null || true

if [[ -f "$CONFORMANCE_ROOT/conformance-report.yaml" ]]; then
  echo "Report saved to: $CONFORMANCE_ROOT/conformance-report.yaml"
fi

exit "$RC"
