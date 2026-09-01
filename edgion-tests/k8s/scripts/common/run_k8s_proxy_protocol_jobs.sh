#!/usr/bin/env bash

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/../../../.." && pwd)"
K8S_DIR="${PROJECT_ROOT}/edgion-tests/k8s/kubernetes/isolated"
FOUNDATION_TEMPLATE="${K8S_DIR}/proxy-foundation.template.yaml"
WORKLOADS_TEMPLATE="${K8S_DIR}/proxy-workloads.template.yaml"
CLIENT_TEMPLATE="${K8S_DIR}/proxy-client-job.template.yaml"
GATEWAY_CLASS_TEMPLATE="${K8S_DIR}/proxy-gateway-class.template.yaml"
CONTROLLER_CONFIG="${K8S_DIR}/proxy-controller-config.yaml"
GATEWAY_CONFIG="${K8S_DIR}/proxy-gateway-config.yaml"

KUBECTL_BIN="${KUBECTL_BIN:-kubectl}"
ISOLATED_IMAGE="${EDGION_ISOLATED_JOB_IMAGE:-}"
HAPROXY_IMAGE="${EDGION_HAPROXY_IMAGE:-haproxy:3.0-alpine}"
ISOLATED_PULL_POLICY="${EDGION_ISOLATED_IMAGE_PULL_POLICY:-IfNotPresent}"
HAPROXY_PULL_POLICY="${EDGION_HAPROXY_IMAGE_PULL_POLICY:-IfNotPresent}"
TIMEOUT_SECONDS="${EDGION_PROXY_TOPOLOGY_TIMEOUT_SECONDS:-420}"
ARTIFACTS_ROOT=""
KEEP_ON_FAILURE=false
REQUESTED_IDS=()
SUPPORTED_IDS=(proxy_protocol_inbound proxy_protocol_chain)

CURRENT_NAMESPACE=""
CURRENT_RBAC_NAME=""
CURRENT_GATEWAY_CLASS=""
CURRENT_GATEWAY_CONFIG=""
CURRENT_FAILED=0
CURRENT_CLEANED=true
RUN_FAILED=0
FAILED_COUNT=0
INFRASTRUCTURE_FAILED=0
JOB_STATE=""
JOB_POD=""
JOB_POD_RC=""
JOB_LOG=""
POD_CIDRS=""

usage() {
  cat <<'EOF'
Usage: run_k8s_proxy_protocol_jobs.sh --image IMAGE [options]

Run the two canonical PROXY protocol scenarios in separate multi-Pod namespaces.

Options:
  --image IMAGE                 Dedicated Edgion isolated-test image
  --haproxy-image IMAGE         HAProxy reference image (default: haproxy:3.0-alpine)
  --job ID                      proxy_protocol_inbound or proxy_protocol_chain; repeatable
  --timeout-seconds N           Per-scenario topology deadline
  --artifacts-dir PATH          Host directory for rendered files and diagnostics
  --image-pull-policy POLICY    Pull policy for the Edgion image
  --haproxy-pull-policy POLICY  Pull policy for the HAProxy image
  --keep-on-failure             Preserve a failed scenario namespace
  -h, --help                    Show this help

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

valid_image() {
  printf '%s\n' "$1" | grep -Eq '^[A-Za-z0-9._/@:-]+$'
}

valid_pull_policy() {
  case "$1" in
    Always|IfNotPresent|Never) return 0 ;;
    *) return 1 ;;
  esac
}

while [ "$#" -gt 0 ]; do
  case "$1" in
    --image)
      [ "$#" -ge 2 ] || die "--image requires a value"
      ISOLATED_IMAGE="$2"
      shift 2
      ;;
    --haproxy-image)
      [ "$#" -ge 2 ] || die "--haproxy-image requires a value"
      HAPROXY_IMAGE="$2"
      shift 2
      ;;
    --job)
      [ "$#" -ge 2 ] || die "--job requires an id"
      REQUESTED_IDS+=("$2")
      shift 2
      ;;
    --timeout-seconds)
      [ "$#" -ge 2 ] || die "--timeout-seconds requires a value"
      TIMEOUT_SECONDS="$2"
      shift 2
      ;;
    --artifacts-dir)
      [ "$#" -ge 2 ] || die "--artifacts-dir requires a path"
      ARTIFACTS_ROOT="$2"
      shift 2
      ;;
    --image-pull-policy)
      [ "$#" -ge 2 ] || die "--image-pull-policy requires a value"
      ISOLATED_PULL_POLICY="$2"
      shift 2
      ;;
    --haproxy-pull-policy)
      [ "$#" -ge 2 ] || die "--haproxy-pull-policy requires a value"
      HAPROXY_PULL_POLICY="$2"
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

[ -n "$ISOLATED_IMAGE" ] || die "--image is required"
valid_image "$ISOLATED_IMAGE" || die "isolated image contains unsupported characters"
valid_image "$HAPROXY_IMAGE" || die "HAProxy image contains unsupported characters"
valid_pull_policy "$ISOLATED_PULL_POLICY" || die "invalid isolated image pull policy"
valid_pull_policy "$HAPROXY_PULL_POLICY" || die "invalid HAProxy image pull policy"
is_positive_integer "$TIMEOUT_SECONDS" || die "timeout must be a positive integer"
for required_file in \
  "$FOUNDATION_TEMPLATE" "$WORKLOADS_TEMPLATE" "$CLIENT_TEMPLATE" \
  "$GATEWAY_CLASS_TEMPLATE" \
  "$CONTROLLER_CONFIG" "$GATEWAY_CONFIG" \
  "${K8S_DIR}/proxy-haproxy-inbound.cfg" "${K8S_DIR}/proxy-haproxy-chain.cfg" \
  "${PROJECT_ROOT}/edgion-tests/integration/conf/ProxyProtocolInbound/Gateway_edgion-test_ppin-gw.yaml" \
  "${PROJECT_ROOT}/edgion-tests/integration/conf/ProxyProtocolInbound/HTTPRoute_edgion-test_ppin-route.yaml" \
  "${PROJECT_ROOT}/edgion-tests/integration/conf/ProxyProtocolInbound/Service_edgion-test_ppin-http.yaml" \
  "${PROJECT_ROOT}/edgion-tests/integration/conf/ProxyProtocolChain/EdgionGatewayConfig__chain-egc.yaml" \
  "${PROJECT_ROOT}/edgion-tests/integration/conf/ProxyProtocolChain/Gateway_edgion-test_chain-gw1.yaml" \
  "${PROJECT_ROOT}/edgion-tests/integration/conf/ProxyProtocolChain/Gateway_edgion-test_chain-gw2.yaml" \
  "${PROJECT_ROOT}/edgion-tests/integration/conf/ProxyProtocolChain/HTTPRoute_edgion-test_chain-r1.yaml" \
  "${PROJECT_ROOT}/edgion-tests/integration/conf/ProxyProtocolChain/HTTPRoute_edgion-test_chain-r2.yaml" \
  "${PROJECT_ROOT}/edgion-tests/integration/conf/ProxyProtocolChain/Service_edgion-test_chain-backend.yaml" \
  "${PROJECT_ROOT}/edgion-tests/integration/conf/ProxyProtocolChain/Service_edgion-test_chain-hop2.yaml"; do
  [ -f "$required_file" ] || die "required topology asset is missing: ${required_file}"
done
command -v "$KUBECTL_BIN" >/dev/null 2>&1 || die "kubectl command not found: ${KUBECTL_BIN}"
"$KUBECTL_BIN" version --client >/dev/null 2>&1 || die "kubectl client preflight failed"
POD_CIDRS="$({
  "$KUBECTL_BIN" get nodes \
    -o jsonpath='{range .items[*]}{.spec.podCIDRs[*]}{"\n"}{end}' 2>/dev/null || true
  "$KUBECTL_BIN" get nodes \
    -o jsonpath='{range .items[*]}{.spec.podCIDR}{"\n"}{end}' 2>/dev/null || true
} | tr ' ' '\n' | awk 'NF' | sort -u)"
[ -n "$POD_CIDRS" ] || die "cluster reports no Pod CIDRs"
while IFS= read -r pod_cidr; do
  printf '%s\n' "$pod_cidr" | grep -Eq '^[0-9a-fA-F:.]+/[0-9]+$' \
    || die "cluster reports an invalid Pod CIDR: ${pod_cidr}"
done <<EOF
$POD_CIDRS
EOF

if [ "${#REQUESTED_IDS[@]}" -eq 0 ]; then
  REQUESTED_IDS=("${SUPPORTED_IDS[@]}")
fi
for requested_id in "${REQUESTED_IDS[@]}"; do
  supported=false
  duplicate_count=0
  for supported_id in "${SUPPORTED_IDS[@]}"; do
    [ "$requested_id" != "$supported_id" ] || supported=true
  done
  $supported || die "unsupported K8s PROXY protocol scenario: ${requested_id}"
  for other_id in "${REQUESTED_IDS[@]}"; do
    [ "$requested_id" != "$other_id" ] || duplicate_count=$((duplicate_count + 1))
  done
  [ "$duplicate_count" -eq 1 ] || die "duplicate requested scenario: ${requested_id}"
done

run_stamp="$(date -u +%Y%m%d%H%M%S)"
if [ -z "$ARTIFACTS_ROOT" ]; then
  ARTIFACTS_ROOT="${PROJECT_ROOT}/tmp/integration_testing/k8s-proxy-isolated-${run_stamp}-$$"
fi
mkdir -p "$ARTIFACTS_ROOT"

cleanup_current_namespace() {
  local cleanup_failed=0
  if $CURRENT_CLEANED || [ -z "$CURRENT_NAMESPACE" ]; then
    return 0
  fi
  if $KEEP_ON_FAILURE && [ "$CURRENT_FAILED" -ne 0 ]; then
    echo "INFO: preserving failed topology ${CURRENT_NAMESPACE}, including its run-owned cluster resources"
    CURRENT_CLEANED=true
    return 0
  fi
  case "$CURRENT_NAMESPACE" in
    edgion-proxy-isolated-*) ;;
    *) echo "ERROR: refusing to delete unsafe namespace ${CURRENT_NAMESPACE}" >&2; return 1 ;;
  esac
  case "$CURRENT_RBAC_NAME" in edgion-proxy-isolated-*) ;; *) return 1 ;; esac
  case "$CURRENT_GATEWAY_CLASS" in edgion-proxy-isolated-*) ;; *) return 1 ;; esac
  if [ -n "$CURRENT_GATEWAY_CONFIG" ]; then
    case "$CURRENT_GATEWAY_CONFIG" in edgion-proxy-isolated-*) ;; *) return 1 ;; esac
    "$KUBECTL_BIN" delete edgiongatewayclassconfig "$CURRENT_GATEWAY_CONFIG" \
      --ignore-not-found --wait=false >/dev/null 2>&1 || cleanup_failed=1
  fi
  "$KUBECTL_BIN" delete gatewayclass "$CURRENT_GATEWAY_CLASS" \
    --ignore-not-found --wait=false >/dev/null 2>&1 || cleanup_failed=1
  "$KUBECTL_BIN" delete namespace "$CURRENT_NAMESPACE" \
    --wait=false >/dev/null 2>&1 || cleanup_failed=1
  "$KUBECTL_BIN" delete clusterrolebinding "$CURRENT_RBAC_NAME" \
    --ignore-not-found --wait=false >/dev/null 2>&1 || cleanup_failed=1
  "$KUBECTL_BIN" delete clusterrole "$CURRENT_RBAC_NAME" \
    --ignore-not-found --wait=false >/dev/null 2>&1 || cleanup_failed=1
  if [ "$cleanup_failed" -ne 0 ]; then
    echo "ERROR: failed to delete one or more run-owned resources for ${CURRENT_NAMESPACE}" >&2
    return 1
  fi
  CURRENT_CLEANED=true
  return 0
}

abort_cleanup() {
  cleanup_current_namespace >/dev/null 2>&1 || true
}
trap abort_cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

render_common() {
  local source_file="$1" output_file="$2" namespace="$3" run_id="$4" test_server_port="$5"
  sed \
    -e "s|__NAMESPACE__|${namespace}|g" \
    -e "s|__RUN_ID__|${run_id}|g" \
    -e "s|__ISOLATED_IMAGE__|${ISOLATED_IMAGE}|g" \
    -e "s|__ISOLATED_PULL_POLICY__|${ISOLATED_PULL_POLICY}|g" \
    -e "s|__HAPROXY_IMAGE__|${HAPROXY_IMAGE}|g" \
    -e "s|__HAPROXY_PULL_POLICY__|${HAPROXY_PULL_POLICY}|g" \
    -e "s|__TEST_SERVER_PORT__|${test_server_port}|g" \
    -e "s|__RBAC_NAME__|${CURRENT_RBAC_NAME}|g" \
    "$source_file" > "$output_file"
}

job_pod_name() {
  local namespace="$1" job_name="$2"
  "$KUBECTL_BIN" get pods -n "$namespace" -l "job-name=${job_name}" \
    -o jsonpath='{.items[0].metadata.name}' 2>/dev/null || true
}

wait_for_job_terminal() {
  local namespace="$1" job_name="$2" deadline_epoch="$3" succeeded failed now reason
  JOB_STATE=""
  while :; do
    succeeded="$("$KUBECTL_BIN" get job "$job_name" -n "$namespace" \
      -o jsonpath='{.status.succeeded}' 2>/dev/null || true)"
    failed="$("$KUBECTL_BIN" get job "$job_name" -n "$namespace" \
      -o jsonpath='{.status.failed}' 2>/dev/null || true)"
    case "$succeeded" in
      ''|*[!0-9]*) ;;
      *) if [ "$succeeded" -gt 0 ]; then JOB_STATE=succeeded; break; fi ;;
    esac
    case "$failed" in
      ''|*[!0-9]*) ;;
      *)
        if [ "$failed" -gt 0 ]; then
          reason="$("$KUBECTL_BIN" get job "$job_name" -n "$namespace" \
            -o jsonpath='{.status.conditions[?(@.type=="Failed")].reason}' 2>/dev/null || true)"
          if [ "$reason" = "DeadlineExceeded" ]; then JOB_STATE=timed_out; else JOB_STATE=failed; fi
          break
        fi
        ;;
    esac
    now="$(date +%s)"
    if [ "$now" -ge "$deadline_epoch" ]; then
      JOB_STATE=timed_out
      break
    fi
    sleep 2
  done
  JOB_POD="$(job_pod_name "$namespace" "$job_name")"
  JOB_POD_RC=""
  if [ -n "$JOB_POD" ]; then
    JOB_POD_RC="$("$KUBECTL_BIN" get pod "$JOB_POD" -n "$namespace" \
      -o jsonpath='{.status.containerStatuses[0].state.terminated.exitCode}' 2>/dev/null || true)"
  fi
}

validate_bootstrap_job() {
  local namespace="$1" artifact_dir="$2" deadline_epoch="$3" marker_count marker_line
  wait_for_job_terminal "$namespace" edgion-proxy-cert-bootstrap "$deadline_epoch"
  JOB_LOG="${artifact_dir}/cert-bootstrap.log"
  "$KUBECTL_BIN" logs job/edgion-proxy-cert-bootstrap -n "$namespace" > "$JOB_LOG" 2>&1 || true
  marker_count="$(awk '$1 == "@@ISOLATED_BOOTSTRAP_RESULT" { count += 1 } END { print count + 0 }' "$JOB_LOG")"
  marker_line="$(awk '$1 == "@@ISOLATED_BOOTSTRAP_RESULT" && NF == 3 && $2 == "PASS" && $3 == "0" { print }' "$JOB_LOG")"
  [ "$JOB_STATE" = succeeded ] \
    && [ "$JOB_POD_RC" = 0 ] \
    && [ "$marker_count" -eq 1 ] \
    && [ "$marker_line" = '@@ISOLATED_BOOTSTRAP_RESULT PASS 0' ]
}

wait_for_deployments() {
  local namespace="$1" deadline_epoch="$2" deployment available now all_ready
  while :; do
    all_ready=true
    for deployment in \
      edgion-proxy-controller edgion-proxy-gateway edgion-proxy-test-server \
      edgion-proxy-wire-dump edgion-proxy-haproxy; do
      available="$("$KUBECTL_BIN" get deployment "$deployment" -n "$namespace" \
        -o jsonpath='{.status.availableReplicas}' 2>/dev/null || true)"
      [ "$available" = 1 ] || all_ready=false
    done
    $all_ready && return 0
    now="$(date +%s)"
    [ "$now" -lt "$deadline_epoch" ] || return 1
    sleep 2
  done
}

collect_diagnostics() {
  local namespace="$1" artifact_dir="$2" pod
  "$KUBECTL_BIN" get all -n "$namespace" -o yaml > "${artifact_dir}/all.yaml" 2>&1 || true
  "$KUBECTL_BIN" get gatewayclass "$CURRENT_GATEWAY_CLASS" -o yaml \
    > "${artifact_dir}/gateway-class.live.yaml" 2>&1 || true
  if [ -n "$CURRENT_GATEWAY_CONFIG" ]; then
    "$KUBECTL_BIN" get edgiongatewayclassconfig "$CURRENT_GATEWAY_CONFIG" -o yaml \
      > "${artifact_dir}/gateway-config.live.yaml" 2>&1 || true
  fi
  "$KUBECTL_BIN" get clusterrole "$CURRENT_RBAC_NAME" -o yaml \
    > "${artifact_dir}/cluster-role.live.yaml" 2>&1 || true
  "$KUBECTL_BIN" get clusterrolebinding "$CURRENT_RBAC_NAME" -o yaml \
    > "${artifact_dir}/cluster-role-binding.live.yaml" 2>&1 || true
  "$KUBECTL_BIN" get events -n "$namespace" --sort-by=.lastTimestamp \
    > "${artifact_dir}/events.txt" 2>&1 || true
  "$KUBECTL_BIN" get pods -n "$namespace" -o wide > "${artifact_dir}/pods.txt" 2>&1 || true
  for pod in $("$KUBECTL_BIN" get pods -n "$namespace" -o jsonpath='{.items[*].metadata.name}' 2>/dev/null || true); do
    "$KUBECTL_BIN" describe pod "$pod" -n "$namespace" \
      > "${artifact_dir}/${pod}.describe.txt" 2>&1 || true
    "$KUBECTL_BIN" logs "$pod" -n "$namespace" --all-containers=true \
      > "${artifact_dir}/${pod}.log" 2>&1 || true
  done
}

validate_client_result() {
  local scenario_id="$1" namespace="$2" artifact_dir="$3" deadline_epoch="$4"
  local marker_count marker_valid marker_status marker_rc gateway_log
  RESULT_STATUS=FAIL
  RESULT_RC=125
  wait_for_job_terminal "$namespace" edgion-proxy-client "$deadline_epoch"
  JOB_LOG="${artifact_dir}/client.log"
  "$KUBECTL_BIN" logs job/edgion-proxy-client -n "$namespace" > "$JOB_LOG" 2>&1 || true
  cat "$JOB_LOG"
  if [ "$JOB_STATE" = timed_out ]; then
    RESULT_RC=124
    return 0
  fi
  marker_count="$(awk '$1 == "@@ISOLATED_CONTAINER_RESULT" { count += 1 } END { print count + 0 }' "$JOB_LOG")"
  marker_valid="$(awk -v id="$scenario_id" '
    $1 == "@@ISOLATED_CONTAINER_RESULT" && NF == 4 && $2 == id \
      && ($3 == "PASS" || $3 == "FAIL") && $4 ~ /^[0-9]+$/ { print $3 " " $4 }
  ' "$JOB_LOG")"
  if [ "$marker_count" -ne 1 ] || [ -z "$marker_valid" ] \
    || ! printf '%s\n' "$JOB_POD_RC" | grep -Eq '^[0-9]+$'; then
    return 0
  fi
  set -- $marker_valid
  marker_status="$1"
  marker_rc="$2"
  [ "$marker_rc" -eq "$JOB_POD_RC" ] || return 0
  if [ "$marker_status" = PASS ] && [ "$marker_rc" -eq 0 ] && [ "$JOB_STATE" = succeeded ]; then
    gateway_log="${artifact_dir}/gateway.log"
    "$KUBECTL_BIN" logs deployment/edgion-proxy-gateway -n "$namespace" > "$gateway_log" 2>&1 || true
    if grep -q 'Inbound PROXY protocol enabled' "$gateway_log"; then
      RESULT_STATUS=PASS
      RESULT_RC=0
    fi
  elif [ "$marker_status" = FAIL ] && [ "$marker_rc" -ne 0 ] && [ "$JOB_STATE" = failed ]; then
    RESULT_RC="$marker_rc"
  fi
}

run_scenario() {
  local scenario_id="$1" scenario_suffix test_server_port fixture_source haproxy_config haproxy_ready_port
  local scenario_dir run_id foundation_file workloads_file client_file rendered_conf
  local rendered_controller_config gateway_class_file parameters_ref_file cidr_file fixture_file
  local service_selector
  local deadline_epoch client_deadline=180

  case "$scenario_id" in
    proxy_protocol_inbound)
      scenario_suffix=inbound
      test_server_port=30061
      fixture_source="${PROJECT_ROOT}/edgion-tests/integration/conf/ProxyProtocolInbound"
      haproxy_config="${K8S_DIR}/proxy-haproxy-inbound.cfg"
      haproxy_ready_port=58440
      ;;
    proxy_protocol_chain)
      scenario_suffix=chain
      test_server_port=30062
      fixture_source="${PROJECT_ROOT}/edgion-tests/integration/conf/ProxyProtocolChain"
      haproxy_config="${K8S_DIR}/proxy-haproxy-chain.cfg"
      haproxy_ready_port=58540
      ;;
    *) return 2 ;;
  esac

  scenario_dir="${ARTIFACTS_ROOT}/${scenario_id}"
  mkdir -p "$scenario_dir"
  run_id="proxy-${run_stamp}-$$-${scenario_suffix}"
  CURRENT_NAMESPACE="edgion-proxy-isolated-${run_stamp}-$$-${scenario_suffix}"
  CURRENT_RBAC_NAME="$CURRENT_NAMESPACE"
  CURRENT_GATEWAY_CLASS="$CURRENT_NAMESPACE"
  CURRENT_GATEWAY_CONFIG=""
  if [ "$scenario_id" = proxy_protocol_chain ]; then
    CURRENT_GATEWAY_CONFIG="${CURRENT_NAMESPACE}-config"
  fi
  CURRENT_FAILED=0
  CURRENT_CLEANED=false
  deadline_epoch=$(( $(date +%s) + TIMEOUT_SECONDS ))
  foundation_file="${scenario_dir}/foundation.yaml"
  workloads_file="${scenario_dir}/workloads.yaml"
  client_file="${scenario_dir}/client.yaml"
  rendered_conf="${scenario_dir}/conf"
  rendered_controller_config="${scenario_dir}/controller.yaml"
  gateway_class_file="${scenario_dir}/gateway-class.yaml"

  echo "INFO: starting ${scenario_id} in ${CURRENT_NAMESPACE}"
  "$KUBECTL_BIN" create namespace "$CURRENT_NAMESPACE" >/dev/null || return 125
  "$KUBECTL_BIN" label namespace "$CURRENT_NAMESPACE" \
    "edgion.io/test-run=${run_id}" \
    "app.kubernetes.io/managed-by=edgion-k8s-proxy-runner" >/dev/null || return 125

  render_common "$FOUNDATION_TEMPLATE" "$foundation_file" \
    "$CURRENT_NAMESPACE" "$run_id" "$test_server_port"
  "$KUBECTL_BIN" apply -f "$foundation_file" >/dev/null || return 125
  validate_bootstrap_job "$CURRENT_NAMESPACE" "$scenario_dir" "$deadline_epoch" || return 125

  mkdir -p "$rendered_conf"
  if [ "$scenario_id" = proxy_protocol_inbound ]; then
    for fixture_file in \
      Gateway_edgion-test_ppin-gw.yaml \
      HTTPRoute_edgion-test_ppin-route.yaml \
      Service_edgion-test_ppin-http.yaml; do
      cp "${fixture_source}/${fixture_file}" "${rendered_conf}/${fixture_file}"
    done
  else
    for fixture_file in \
      EdgionGatewayConfig__chain-egc.yaml \
      Gateway_edgion-test_chain-gw1.yaml Gateway_edgion-test_chain-gw2.yaml \
      HTTPRoute_edgion-test_chain-r1.yaml HTTPRoute_edgion-test_chain-r2.yaml \
      Service_edgion-test_chain-backend.yaml Service_edgion-test_chain-hop2.yaml; do
      cp "${fixture_source}/${fixture_file}" "${rendered_conf}/${fixture_file}"
    done
  fi

  for fixture_file in "$rendered_conf"/*.yaml; do
    sed \
      -e "s|namespace: edgion-test|namespace: ${CURRENT_NAMESPACE}|g" \
      -e "s|gatewayClassName: public-gateway|gatewayClassName: ${CURRENT_GATEWAY_CLASS}|g" \
      -e "s|name: chain-egc|name: ${CURRENT_GATEWAY_CONFIG}|g" \
      "$fixture_file" > "${fixture_file}.tmp"
    mv "${fixture_file}.tmp" "$fixture_file"
  done
  for fixture_file in "$rendered_conf"/Service_*.yaml; do
    case "$fixture_file" in
      *chain-hop2*) service_selector=edgion-proxy-gateway ;;
      *) service_selector=edgion-proxy-test-server ;;
    esac
    awk -v selector="$service_selector" '
      /^spec:$/ {
        print
        print "  clusterIP: None"
        print "  selector:"
        print "    app.kubernetes.io/name: " selector
        next
      }
      { print }
    ' "$fixture_file" > "${fixture_file}.tmp"
    mv "${fixture_file}.tmp" "$fixture_file"
  done
  if [ "$scenario_id" = proxy_protocol_chain ]; then
    cidr_file="${scenario_dir}/pod-cidrs.txt"
    printf '%s\n' "$POD_CIDRS" > "$cidr_file"
    fixture_file="${rendered_conf}/EdgionGatewayConfig__chain-egc.yaml"
    awk -v cidr_file="$cidr_file" '
      /- "127\.0\.0\.0\/8"/ {
        while ((getline cidr < cidr_file) > 0) print "          - \"" cidr "\""
        close(cidr_file)
        next
      }
      { print }
    ' "$fixture_file" > "${fixture_file}.tmp"
    mv "${fixture_file}.tmp" "$fixture_file"
  fi

  parameters_ref_file="${scenario_dir}/gateway-class-parameters.txt"
  if [ "$scenario_id" = proxy_protocol_chain ]; then
    printf '  parametersRef:\n    group: edgion.io\n    kind: EdgionGatewayConfig\n    name: %s\n' \
      "$CURRENT_GATEWAY_CONFIG" > "$parameters_ref_file"
  else
    : > "$parameters_ref_file"
  fi
  awk -v parameters_file="$parameters_ref_file" '
    /^__PARAMETERS_REF__$/ {
      while ((getline line < parameters_file) > 0) print line
      close(parameters_file)
      next
    }
    { print }
  ' "$GATEWAY_CLASS_TEMPLATE" > "${gateway_class_file}.tmp"
  sed \
    -e "s|__GATEWAY_CLASS__|${CURRENT_GATEWAY_CLASS}|g" \
    -e "s|__RUN_ID__|${run_id}|g" \
    "${gateway_class_file}.tmp" > "$gateway_class_file"
  rm -f "${gateway_class_file}.tmp"
  cp "$gateway_class_file" "${rendered_conf}/GatewayClass.yaml"

  sed \
    -e "s|__GATEWAY_CLASS__|${CURRENT_GATEWAY_CLASS}|g" \
    -e "s|__NAMESPACE__|${CURRENT_NAMESPACE}|g" \
    -e "s|__LEASE_NAME__|${CURRENT_NAMESPACE}|g" \
    "$CONTROLLER_CONFIG" > "$rendered_controller_config"
  "$KUBECTL_BIN" create configmap edgion-proxy-controller-config -n "$CURRENT_NAMESPACE" \
    --from-file=controller.yaml="$rendered_controller_config" >/dev/null || return 125
  "$KUBECTL_BIN" create configmap edgion-proxy-gateway-config -n "$CURRENT_NAMESPACE" \
    --from-file=gateway.yaml="$GATEWAY_CONFIG" >/dev/null || return 125
  "$KUBECTL_BIN" create configmap edgion-proxy-haproxy-config -n "$CURRENT_NAMESPACE" \
    --from-file=haproxy.cfg="$haproxy_config" >/dev/null || return 125
  "$KUBECTL_BIN" apply -f "$rendered_conf" >/dev/null || return 125

  render_common "$WORKLOADS_TEMPLATE" "$workloads_file" \
    "$CURRENT_NAMESPACE" "$run_id" "$test_server_port"
  sed -e "s|__HAPROXY_READY_PORT__|${haproxy_ready_port}|g" \
    "$workloads_file" > "${workloads_file}.tmp"
  mv "${workloads_file}.tmp" "$workloads_file"
  "$KUBECTL_BIN" apply -f "$workloads_file" >/dev/null || return 125
  wait_for_deployments "$CURRENT_NAMESPACE" "$deadline_epoch" || return 124

  render_common "$CLIENT_TEMPLATE" "$client_file" \
    "$CURRENT_NAMESPACE" "$run_id" "$test_server_port"
  sed \
    -e "s|__SCENARIO_ID__|${scenario_id}|g" \
    -e "s|__CLIENT_DEADLINE_SECONDS__|${client_deadline}|g" \
    "$client_file" > "${client_file}.tmp"
  mv "${client_file}.tmp" "$client_file"
  "$KUBECTL_BIN" apply -f "$client_file" >/dev/null || return 125
  validate_client_result "$scenario_id" "$CURRENT_NAMESPACE" "$scenario_dir" "$deadline_epoch"
  [ "$RESULT_STATUS" = PASS ] || return "$RESULT_RC"
  return 0
}

for scenario_id in "${REQUESTED_IDS[@]}"; do
  set +e
  run_scenario "$scenario_id"
  scenario_rc=$?
  set -e
  if [ "$scenario_rc" -eq 0 ]; then
    printf '@@ISOLATED_RESULT %s PASS 0\n' "$scenario_id"
  else
    CURRENT_FAILED=1
    RUN_FAILED=1
    FAILED_COUNT=$((FAILED_COUNT + 1))
    collect_diagnostics "$CURRENT_NAMESPACE" "${ARTIFACTS_ROOT}/${scenario_id}"
    if [ "$scenario_rc" -ge 1 ] && [ "$scenario_rc" -le 125 ]; then
      normalized_rc="$scenario_rc"
    else
      normalized_rc=125
    fi
    printf '@@ISOLATED_RESULT %s FAIL %s\n' "$scenario_id" "$normalized_rc"
  fi
  if ! cleanup_current_namespace; then
    INFRASTRUCTURE_FAILED=1
    RUN_FAILED=1
  fi
done
trap - EXIT INT TERM

printf '@@ISOLATED_SUMMARY total=%s failed=%s infrastructure_failed=%s\n' \
  "${#REQUESTED_IDS[@]}" "$FAILED_COUNT" "$INFRASTRUCTURE_FAILED"
[ "$RUN_FAILED" -eq 0 ]
