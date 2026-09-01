#!/usr/bin/env bash

set -euo pipefail

endpoint_inventory_usage() {
  cat <<'EOF'
Usage: endpoint_inventory.sh --namespace <namespace> --service <service> --port <name-or-number> [options]

Resolve ready, non-terminating addresses from discovery.k8s.io/v1 EndpointSlices.

Options:
  --namespace <namespace>    Namespace containing the Service and EndpointSlices.
  --service <service>        Service name from the kubernetes.io/service-name label.
  --port <name-or-number>    EndpointSlice port name or numeric port.
  --expected-count <count>   Wait for exactly this many unique address/port pairs.
  --timeout-secs <seconds>   Overall wait deadline (default: 30; 0 performs one query).
  --interval-secs <seconds>  Poll interval (default: 1).
  --output <mode>            json (default), ips, or hostports.
  -h, --help                 Show this help.

The JSON result is a stable-sorted array of objects with address, port,
portName, protocol, and addressType fields. The ips and hostports modes emit
stable comma-separated values; IPv6 hostports use brackets.
EOF
}

endpoint_inventory_error() {
  echo "[ERROR] $*" >&2
}

endpoint_inventory_require_command() {
  local command_name="$1"
  if ! command -v "$command_name" >/dev/null 2>&1; then
    endpoint_inventory_error "$command_name not found in PATH"
    return 1
  fi
}

endpoint_inventory_validate_dns_label() {
  local value="$1"
  local label="$2"
  case "$value" in
    ""|*[!a-z0-9-]*|-*|*-)
      endpoint_inventory_error "$label is not a valid Kubernetes name: $value"
      return 1
      ;;
  esac
}

endpoint_inventory_validate_uint() {
  local value="$1"
  local label="$2"
  case "$value" in
    ""|*[!0-9]*)
      endpoint_inventory_error "$label must be a non-negative integer"
      return 1
      ;;
  esac
}

# Return the Deployment replica count only when every desired replica is
# currently available. Callers pass the result to --expected-count so a
# generated EndpointSlice inventory cannot silently omit a test-server Pod.
endpoint_inventory_strict_deployment_replicas() {
  local namespace="$1"
  local deployment="$2"
  local replicas desired available

  endpoint_inventory_validate_dns_label "$namespace" "namespace" || return 2
  endpoint_inventory_validate_dns_label "$deployment" "deployment" || return 2
  replicas="$(
    kubectl get deployment "$deployment" \
      --namespace "$namespace" \
      --output 'jsonpath={.spec.replicas}{" "}{.status.availableReplicas}'
  )" || return 1
  read -r desired available <<EOF
$replicas
EOF
  endpoint_inventory_validate_uint "$desired" "desired replica count" || return 1
  endpoint_inventory_validate_uint "$available" "available replica count" || return 1
  if [ "$desired" -eq 0 ]; then
    endpoint_inventory_error "$namespace/$deployment has zero desired replicas"
    return 1
  fi
  if [ "$available" -ne "$desired" ]; then
    endpoint_inventory_error \
      "$namespace/$deployment has $available available replicas, expected $desired"
    return 1
  fi
  printf '%s\n' "$available"
}

# Wait until one fresh EndpointSlice inventory has the exact expected size and
# every selected backend answers its direct health endpoint from the test-client
# Pod. Re-querying the API on every pass avoids pinning a stale Pod IP while a
# Deployment or the cluster network is still converging.
endpoint_inventory_wait_for_backend_health() {
  local namespace="$1"
  local service="$2"
  local port_selector="$3"
  local expected_count="$4"
  local client_namespace="$5"
  local client_pod="$6"
  local client_container="$7"
  local timeout_secs="${8:-90}"
  local deadline now endpoints_csv endpoint last_error output all_healthy

  endpoint_inventory_validate_dns_label "$namespace" "namespace" || return 2
  endpoint_inventory_validate_dns_label "$service" "service" || return 2
  endpoint_inventory_validate_dns_label "$client_namespace" "client namespace" || return 2
  endpoint_inventory_validate_dns_label "$client_pod" "client Pod" || return 2
  endpoint_inventory_validate_dns_label "$client_container" "client container" || return 2
  endpoint_inventory_validate_uint "$expected_count" "expected endpoint count" || return 2
  endpoint_inventory_validate_uint "$timeout_secs" "backend health timeout" || return 2
  if [ "$expected_count" -eq 0 ] || [ "$timeout_secs" -eq 0 ]; then
    endpoint_inventory_error "expected endpoint count and backend health timeout must be positive"
    return 2
  fi

  deadline=$(( $(date +%s) + timeout_secs ))
  last_error="EndpointSlice inventory has not completed"
  while :; do
    if endpoints_csv=$(endpoint_inventory_main \
      --namespace "$namespace" \
      --service "$service" \
      --port "$port_selector" \
      --expected-count "$expected_count" \
      --timeout-secs 0 \
      --output hostports 2>&1); then
      all_healthy=true
      last_error="health request did not run"
      while IFS= read -r endpoint; do
        [ -n "$endpoint" ] || continue
        if output=$(kubectl --request-timeout=5s exec \
          --namespace "$client_namespace" \
          --container "$client_container" \
          "$client_pod" -- \
          curl --fail --silent --show-error --connect-timeout 2 --max-time 3 \
          "http://${endpoint}/health" 2>&1); then
          :
        else
          all_healthy=false
          last_error="$namespace/$service endpoint $endpoint: $output"
        fi
      done <<EOF
$(printf '%s\n' "$endpoints_csv" | tr ',' '\n')
EOF
      if [ "$all_healthy" = true ]; then
        echo "Backend health precondition passed: ${namespace}/${service} endpoints ${endpoints_csv}" >&2
        printf '%s\n' "$endpoints_csv"
        return 0
      fi
    else
      last_error="$endpoints_csv"
    fi

    now=$(date +%s)
    if [ "$now" -ge "$deadline" ]; then
      endpoint_inventory_error \
        "timed out after ${timeout_secs}s waiting for healthy ${namespace}/${service}: ${last_error}"
      return 1
    fi
    sleep 1
  done
}

endpoint_inventory_query() {
  local namespace="$1"
  local service="$2"
  local request_timeout_secs="$3"
  # `kubectl get <resource> -o json` rewrites a collection to the generic
  # `v1/List` envelope on some kubectl versions. Query the discovery endpoint
  # directly so strict type validation below receives the API server's native
  # `discovery.k8s.io/v1 EndpointSliceList` representation.
  kubectl get --raw \
    "/apis/discovery.k8s.io/v1/namespaces/${namespace}/endpointslices?labelSelector=kubernetes.io%2Fservice-name%3D${service}" \
    --request-timeout "${request_timeout_secs}s"
}

endpoint_inventory_normalize() {
  local selector_kind="$1"
  local selector_value="$2"

  jq -c \
    --arg selector_kind "$selector_kind" \
    --arg selector_value "$selector_value" '
      def selected_port:
        if $selector_kind == "number" then
          (.port | type == "number") and (.port == ($selector_value | tonumber))
        else
          .name == $selector_value
        end;

      def valid_ipv4:
        split(".") as $parts
        | ($parts | length) == 4
        and all($parts[]; test("^[0-9]+$") and ((tonumber >= 0) and (tonumber <= 255)));

      def valid_hextet:
        test("^[0-9A-Fa-f]{1,4}$");

      def hextets:
        if . == "" then [] else split(":") end;

      def valid_ipv6:
        test("^[0-9A-Fa-f:]+$")
        and
        (if contains("::") then
           split("::") as $halves
           | ($halves | length) == 2
             and (($halves[0] | hextets) as $left
                  | ($halves[1] | hextets) as $right
                  | (($left | length) + ($right | length)) < 8
                    and all(($left + $right)[]; valid_hextet))
         else
           split(":") as $groups
           | ($groups | length) == 8 and all($groups[]; valid_hextet)
         end);

      def valid_fqdn:
        test("^([A-Za-z0-9]([A-Za-z0-9-]*[A-Za-z0-9])?)([.]([A-Za-z0-9]([A-Za-z0-9-]*[A-Za-z0-9])?))*[.]?$");

      def valid_address($address_type):
        type == "string"
        and length > 0
        and (test("[[:space:]]") | not)
        and (if $address_type == "IPv4" then valid_ipv4
             elif $address_type == "IPv6" then valid_ipv6
             elif $address_type == "FQDN" then valid_fqdn
             else false
             end);

      if .apiVersion != "discovery.k8s.io/v1"
         or .kind != "EndpointSliceList"
         or (.items | type) != "array"
      then error("expected a discovery.k8s.io/v1 EndpointSliceList with an items array")
      else
        [.items[]? as $slice
         | $slice.ports[]?
         | select(selected_port)
         | {slice: ($slice.metadata.name // ""),
            addressType: ($slice.addressType // ""),
            name: (.name // null),
            port: .port,
            protocol: (.protocol // "TCP")}] as $ports
        | [.items[]? as $slice
           | $slice.ports[]? as $port
           | select($port | selected_port)
           | $slice.endpoints[]?
           | select(.conditions.ready != false and .conditions.terminating != true)
           | {addressType: ($slice.addressType // ""), addresses: .addresses,
              portName: ($port.name // null), port: $port.port,
              protocol: ($port.protocol // "TCP")}] as $ready
        | {
            matchedPortCount: ($ports | length),
            invalidPortCount: ([$ports[]
              | select(if (.port | type) != "number" then true
                       else (.port | floor) != .port or .port < 1 or .port > 65535
                       end)] | length),
            invalidEndpointCount: ([$ready[] as $endpoint
              | select(($endpoint.addresses | type) != "array"
                       or ($endpoint.addresses | length) == 0
                       or any($endpoint.addresses[];
                              (valid_address($endpoint.addressType) | not)))] | length),
            endpoints: ([$ready[]
              | select((.addresses | type) == "array" and (.addresses | length) > 0)
              | . as $endpoint
              | .addresses[]
              | {address: ., port: $endpoint.port, portName: $endpoint.portName,
                 protocol: $endpoint.protocol, addressType: $endpoint.addressType}]
              | unique_by([.addressType, .address, .port, (.portName // ""), .protocol])
              | sort_by([.addressType, .address, .port, (.portName // ""), .protocol]))
          }
      end
    '
}

endpoint_inventory_render() {
  local normalized="$1"
  local mode="$2"

  case "$mode" in
    json)
      printf '%s\n' "$normalized" | jq -c '.endpoints'
      ;;
    ips)
      printf '%s\n' "$normalized" | jq -r '[.endpoints[].address] | unique | sort | join(",")'
      ;;
    hostports)
      printf '%s\n' "$normalized" | jq -r '
        [.endpoints[]
         | (if .addressType == "IPv6" then "[\(.address)]:\(.port)" else "\(.address):\(.port)" end)]
        | unique | sort | join(",")'
      ;;
  esac
}

endpoint_inventory_main() {
  local namespace=""
  local service=""
  local port_selector=""
  local expected_count=""
  local timeout_secs=30
  local interval_secs=1
  local output_mode=json

  while [ "$#" -gt 0 ]; do
    case "$1" in
      --namespace)
        [ "$#" -ge 2 ] || { endpoint_inventory_error "missing value for --namespace"; return 2; }
        namespace="$2"
        shift 2
        ;;
      --service)
        [ "$#" -ge 2 ] || { endpoint_inventory_error "missing value for --service"; return 2; }
        service="$2"
        shift 2
        ;;
      --port)
        [ "$#" -ge 2 ] || { endpoint_inventory_error "missing value for --port"; return 2; }
        port_selector="$2"
        shift 2
        ;;
      --expected-count)
        [ "$#" -ge 2 ] || { endpoint_inventory_error "missing value for --expected-count"; return 2; }
        expected_count="$2"
        shift 2
        ;;
      --timeout-secs)
        [ "$#" -ge 2 ] || { endpoint_inventory_error "missing value for --timeout-secs"; return 2; }
        timeout_secs="$2"
        shift 2
        ;;
      --interval-secs)
        [ "$#" -ge 2 ] || { endpoint_inventory_error "missing value for --interval-secs"; return 2; }
        interval_secs="$2"
        shift 2
        ;;
      --output)
        [ "$#" -ge 2 ] || { endpoint_inventory_error "missing value for --output"; return 2; }
        output_mode="$2"
        shift 2
        ;;
      -h|--help)
        endpoint_inventory_usage
        return 0
        ;;
      *)
        endpoint_inventory_error "unknown option: $1"
        endpoint_inventory_usage >&2
        return 2
        ;;
    esac
  done

  if [ -z "$namespace" ] || [ -z "$service" ] || [ -z "$port_selector" ]; then
    endpoint_inventory_error "--namespace, --service, and --port are required"
    return 2
  fi

  endpoint_inventory_require_command kubectl || return 2
  endpoint_inventory_require_command jq || return 2
  endpoint_inventory_validate_dns_label "$namespace" "namespace" || return 2
  endpoint_inventory_validate_dns_label "$service" "service" || return 2
  endpoint_inventory_validate_uint "$timeout_secs" "--timeout-secs" || return 2
  endpoint_inventory_validate_uint "$interval_secs" "--interval-secs" || return 2
  if [ -n "$expected_count" ]; then
    endpoint_inventory_validate_uint "$expected_count" "--expected-count" || return 2
  fi
  case "$output_mode" in
    json|ips|hostports) ;;
    *) endpoint_inventory_error "--output must be json, ips, or hostports"; return 2 ;;
  esac

  local selector_kind=name
  case "$port_selector" in
    *[!0-9]*)
      endpoint_inventory_validate_dns_label "$port_selector" "port name" || return 2
      ;;
    *)
      selector_kind=number
      if [ "$port_selector" -lt 1 ] || [ "$port_selector" -gt 65535 ]; then
        endpoint_inventory_error "numeric --port must be between 1 and 65535"
        return 2
      fi
      ;;
  esac

  local started_at deadline now raw normalized remaining_secs sleep_secs request_timeout_secs
  local matched_port_count invalid_port_count invalid_endpoint_count actual_count
  local last_reason="EndpointSlice query has not completed"
  started_at=$(date +%s)
  deadline=$((started_at + timeout_secs))

  while :; do
    now=$(date +%s)
    request_timeout_secs=$((deadline - now))
    if [ "$request_timeout_secs" -lt 1 ]; then
      request_timeout_secs=1
    fi
    if raw=$(endpoint_inventory_query "$namespace" "$service" "$request_timeout_secs" 2>/dev/null); then
      if normalized=$(printf '%s\n' "$raw" | endpoint_inventory_normalize "$selector_kind" "$port_selector" 2>/dev/null); then
        matched_port_count=$(printf '%s\n' "$normalized" | jq -r '.matchedPortCount')
        invalid_port_count=$(printf '%s\n' "$normalized" | jq -r '.invalidPortCount')
        invalid_endpoint_count=$(printf '%s\n' "$normalized" | jq -r '.invalidEndpointCount')
        actual_count=$(printf '%s\n' "$normalized" | jq -r '.endpoints | length')

        if [ "$invalid_port_count" -gt 0 ]; then
          endpoint_inventory_error "EndpointSlice contains an invalid selected port"
          return 1
        fi
        if [ "$invalid_endpoint_count" -gt 0 ]; then
          endpoint_inventory_error "EndpointSlice contains an invalid ready endpoint address"
          return 1
        fi
        if [ "$matched_port_count" -eq 0 ] && [ "$expected_count" = "0" ]; then
          endpoint_inventory_render "$normalized" "$output_mode"
          return 0
        elif [ "$matched_port_count" -eq 0 ]; then
          last_reason="no EndpointSlice port matches ${port_selector}"
        elif [ -n "$expected_count" ] && [ "$actual_count" -ne "$expected_count" ]; then
          last_reason="expected ${expected_count} endpoints but found ${actual_count}"
        elif [ -z "$expected_count" ] && [ "$actual_count" -eq 0 ]; then
          last_reason="no ready, non-terminating endpoints found"
        else
          endpoint_inventory_render "$normalized" "$output_mode"
          return 0
        fi
      else
        last_reason="EndpointSlice response is not valid discovery.k8s.io/v1 JSON"
      fi
    else
      last_reason="kubectl EndpointSlice query failed"
    fi

    now=$(date +%s)
    if [ "$now" -ge "$deadline" ]; then
      endpoint_inventory_error "timed out after ${timeout_secs}s for ${namespace}/${service} port ${port_selector}: ${last_reason}"
      return 1
    fi
    remaining_secs=$((deadline - now))
    sleep_secs="$interval_secs"
    if [ "$sleep_secs" -gt "$remaining_secs" ]; then
      sleep_secs="$remaining_secs"
    fi
    sleep "$sleep_secs"
  done
}

if [ "${BASH_SOURCE[0]}" = "$0" ]; then
  endpoint_inventory_main "$@"
fi
