#!/usr/bin/env bash
# Fixed ports and bind addresses for standalone integration scenarios. LB_*
# ports are bound only inside each scenario's private Docker network. Keep
# host ports below 32768 to avoid the default Linux/macOS ephemeral ranges. Keep
# assignments here so parallel scenarios cannot silently choose the same host
# port, and so the preflight probes the address each service actually binds.
ISOLATED_FIXED_SCENARIOS="controller_auth gateway_admin_auth fips_policy geoip_db_states unknown_config_fields proxy_protocol_policy_lifecycle proxy_protocol_inbound proxy_protocol_chain metrics_lifetime"

isolated_load_ports() {
    local scenario="$1" name value bind policy spec entry
    local ports=()
    case "$scenario" in
        controller_auth)
            ports=(
            NOAUTH_ADMIN_PORT=28200@127.0.0.1 NOAUTH_GRPC_PORT=28201@127.0.0.1
            NOAUTH_PROBE_PORT=28202@127.0.0.1 NOAUTH_METRICS_PORT=28203@127.0.0.1
            ENVAUTH_ADMIN_PORT=28210@127.0.0.1 ENVAUTH_GRPC_PORT=28211@127.0.0.1
            ENVAUTH_PROBE_PORT=28212@127.0.0.1 ENVAUTH_METRICS_PORT=28213@127.0.0.1
            )
            ;;
        gateway_admin_auth)
            ports=(
            GW_ADMIN_PORT=28300@127.0.0.1 GW_PROBE_PORT=28301@127.0.0.1 GW_METRICS_PORT=28302@127.0.0.1
            CTRL_ADMIN_PORT=28303@127.0.0.1 CTRL_GRPC_PORT=28304@127.0.0.1 CTRL_PROBE_PORT=28305@127.0.0.1
            GW_DATAPLANE_PORT=28306@0.0.0.0 CTRL_METRICS_PORT=28307@127.0.0.1
            )
            ;;
        fips_policy)
            ports=(
            GW_ADMIN_PORT=28310@127.0.0.1 GW_PROBE_PORT=28311@127.0.0.1 GW_METRICS_PORT=28312@127.0.0.1
            CTRL_ADMIN_PORT=28313@127.0.0.1 CTRL_GRPC_PORT=28314@127.0.0.1 CTRL_PROBE_PORT=28315@127.0.0.1
            GW_HTTPS_PORT=28316@0.0.0.0 CTRL_METRICS_PORT=28317@127.0.0.1
            )
            ;;
        geoip_db_states)
            ports=(
            CTRL_ADMIN_PORT=28600@127.0.0.1 CTRL_GRPC_PORT=28601@127.0.0.1 CTRL_PROBE_PORT=28602@127.0.0.1
            GW_ADMIN_PORT=28603@127.0.0.1 GW_PROBE_PORT=28604@127.0.0.1 GW_METRICS_PORT=28605@127.0.0.1
            GW_HTTP_PORT=28606@0.0.0.0 GW_TCP_DENY_PORT=28607@0.0.0.0 GW_TCP_ALLOW_PORT=28608@0.0.0.0
            TCP_BACKEND_PORT=28609@127.0.0.1 HTTP_BACKEND_PORT=28611@127.0.0.1@noreuse
            GW_TCP_CIDR_PORT=28612@0.0.0.0 CTRL_METRICS_PORT=28613@127.0.0.1
            UPDATE_SERVER_PORT=28614@0.0.0.0@noreuse
            )
            ;;
        unknown_config_fields)
            ports=(
            CONTROLLER_ADMIN_PORT=28700@127.0.0.1 CONTROLLER_GRPC_PORT=28701@127.0.0.1
            CONTROLLER_PROBE_PORT=28702@127.0.0.1 CONTROLLER_METRICS_PORT=28703@127.0.0.1
            )
            ;;
        proxy_protocol_policy_lifecycle)
            ports=(
            CTRL_ADMIN_PORT=28920@127.0.0.1 CTRL_GRPC_PORT=28921@127.0.0.1 CTRL_PROBE_PORT=28922@127.0.0.1
            GW_ADMIN_PORT=28923@127.0.0.1 GW_PROBE_PORT=28924@127.0.0.1 GW_METRICS_PORT=28925@127.0.0.1
            CTRL_METRICS_PORT=28926@127.0.0.1 LISTENER_PORT=28930@0.0.0.0 TEST_SERVER_PORT=30063@0.0.0.0
            )
            ;;
        proxy_protocol_inbound)
            ports=(
            CTRL_ADMIN_PORT=28420@127.0.0.1 CTRL_GRPC_PORT=28421@127.0.0.1 CTRL_PROBE_PORT=28422@127.0.0.1
            GW_ADMIN_PORT=28423@127.0.0.1 GW_PROBE_PORT=28424@127.0.0.1 GW_METRICS_PORT=28425@127.0.0.1
            CTRL_METRICS_PORT=28426@127.0.0.1 GW_PP_PORT=28430@0.0.0.0 GW_PLAIN_PORT=28431@0.0.0.0
            GW_TRUST_PORT=28432@0.0.0.0 LB_V2_PORT=58440@docker LB_V1_PORT=58441@docker
            LB_V2_PROBE_PORT=58442@docker LB_V1_PROBE_PORT=58443@docker
            V2_DUMP_PORT=28444@0.0.0.0 V1_DUMP_PORT=28445@0.0.0.0 TS_HTTP_PORT=30061@0.0.0.0
            )
            ;;
        proxy_protocol_chain)
            ports=(
            CTRL_ADMIN_PORT=28520@127.0.0.1 CTRL_GRPC_PORT=28521@127.0.0.1 CTRL_PROBE_PORT=28522@127.0.0.1
            GW_ADMIN_PORT=28523@127.0.0.1 GW_PROBE_PORT=28524@127.0.0.1 GW_METRICS_PORT=28525@127.0.0.1
            CTRL_METRICS_PORT=28526@127.0.0.1 GW_PP_PORT=28530@0.0.0.0 GW_HOP2_PORT=28531@0.0.0.0
            LB_V2_PORT=58540@docker LB_V2_PROBE_PORT=58542@docker V2_DUMP_PORT=28544@0.0.0.0
            TS_HTTP_PORT=30062@0.0.0.0
            )
            ;;
        metrics_lifetime)
            ports=(
            CTRL_ADMIN_PORT=29000@127.0.0.1 CTRL_GRPC_PORT=29001@127.0.0.1 CTRL_PROBE_PORT=29002@127.0.0.1
            CTRL_METRICS_PORT=29003@127.0.0.1 GW_ADMIN_PORT=29004@127.0.0.1 GW_PROBE_PORT=29005@127.0.0.1
            GW_METRICS_PORT=29006@127.0.0.1 GW_HTTP_PORT=31149@0.0.0.0 BACKEND_HTTP_PORT=30001@127.0.0.1
            )
            ;;
        *)
            echo "ERROR: unknown isolated port scenario: $scenario" >&2
            return 1
            ;;
    esac
    ISOLATED_PORT_NAMES=""
    ISOLATED_PORT_ROWS=()
    for entry in "${ports[@]}"; do
        IFS=@ read -r spec bind policy <<< "$entry"
        name="${spec%%=*}"
        value="${spec#*=}"
        policy="${policy:-reuse}"
        if [[ ! "$name" =~ ^[A-Z][A-Z0-9_]*$ ]] \
            || [[ ! "$value" =~ ^[0-9]+$ ]] \
            || [ "$value" -lt 1 ] || [ "$value" -gt 65535 ] \
            || { [ "$bind" != "127.0.0.1" ] && [ "$bind" != "0.0.0.0" ] && [ "$bind" != "docker" ]; } \
            || { [ "$policy" != "reuse" ] && [ "$policy" != "noreuse" ]; }; then
            echo "ERROR: missing or invalid isolated port $scenario/$name" >&2
            return 1
        fi
        printf -v "$name" '%s' "$value"
        ISOLATED_PORT_NAMES="${ISOLATED_PORT_NAMES}${ISOLATED_PORT_NAMES:+ }${name}"
        ISOLATED_PORT_ROWS+=("${name}:${value}:${bind}:${policy}")
    done
}

isolated_print_ports() {
    local scenario="$1" row name value bind policy
    isolated_load_ports "$scenario" || return 1
    for row in "${ISOLATED_PORT_ROWS[@]}"; do
        IFS=: read -r name value bind policy <<< "$row"
        printf '%s\t%s\t%s\t%s\t%s\n' "$scenario" "$name" "$value" "$bind" "$policy"
    done
}

isolated_require_ports() {
    local name
    for name in "$@"; do
        if [ -z "${!name:-}" ]; then
            echo "ERROR: required isolated port variable $name is missing" >&2
            return 1
        fi
    done
}

if [ "${BASH_SOURCE[0]}" = "$0" ]; then
    if [ "$#" -ne 1 ]; then
        echo "Usage: $0 <scenario|--list-scenarios>" >&2
        exit 2
    fi
    if [ "$1" = "--list-scenarios" ]; then
        printf '%s\n' $ISOLATED_FIXED_SCENARIOS
        exit 0
    fi
    isolated_print_ports "$1"
fi
