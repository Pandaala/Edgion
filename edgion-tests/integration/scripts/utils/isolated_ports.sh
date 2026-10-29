#!/usr/bin/env bash
# Fixed ports and bind addresses for standalone integration scenarios. LB_*
# ports are bound only inside each scenario's private Docker network. Keep the
# assignments here so parallel scenarios cannot silently choose the same host
# port, and so the preflight probes the address each service actually binds.
ISOLATED_FIXED_SCENARIOS="controller_auth gateway_admin_auth fips_policy geoip_db_states unknown_config_fields proxy_protocol_policy_lifecycle proxy_protocol_inbound proxy_protocol_chain metrics_lifetime"

isolated_load_ports() {
    local scenario="$1" name value bind policy spec entry
    local ports=()
    case "$scenario" in
        controller_auth)
            ports=(
            NOAUTH_ADMIN_PORT=58200@127.0.0.1 NOAUTH_GRPC_PORT=58201@127.0.0.1
            NOAUTH_PROBE_PORT=58202@127.0.0.1 NOAUTH_METRICS_PORT=58203@127.0.0.1
            ENVAUTH_ADMIN_PORT=58210@127.0.0.1 ENVAUTH_GRPC_PORT=58211@127.0.0.1
            ENVAUTH_PROBE_PORT=58212@127.0.0.1 ENVAUTH_METRICS_PORT=58213@127.0.0.1
            )
            ;;
        gateway_admin_auth)
            ports=(
            GW_ADMIN_PORT=58300@127.0.0.1 GW_PROBE_PORT=58301@127.0.0.1 GW_METRICS_PORT=58302@127.0.0.1
            CTRL_ADMIN_PORT=58303@127.0.0.1 CTRL_GRPC_PORT=58304@127.0.0.1 CTRL_PROBE_PORT=58305@127.0.0.1
            GW_DATAPLANE_PORT=58306@0.0.0.0 CTRL_METRICS_PORT=58307@127.0.0.1
            )
            ;;
        fips_policy)
            ports=(
            GW_ADMIN_PORT=58310@127.0.0.1 GW_PROBE_PORT=58311@127.0.0.1 GW_METRICS_PORT=58312@127.0.0.1
            CTRL_ADMIN_PORT=58313@127.0.0.1 CTRL_GRPC_PORT=58314@127.0.0.1 CTRL_PROBE_PORT=58315@127.0.0.1
            GW_HTTPS_PORT=58316@0.0.0.0 CTRL_METRICS_PORT=58317@127.0.0.1
            )
            ;;
        geoip_db_states)
            ports=(
            CTRL_ADMIN_PORT=58600@127.0.0.1 CTRL_GRPC_PORT=58601@127.0.0.1 CTRL_PROBE_PORT=58602@127.0.0.1
            GW_ADMIN_PORT=58603@127.0.0.1 GW_PROBE_PORT=58604@127.0.0.1 GW_METRICS_PORT=58605@127.0.0.1
            GW_HTTP_PORT=58606@0.0.0.0 GW_TCP_DENY_PORT=58607@0.0.0.0 GW_TCP_ALLOW_PORT=58608@0.0.0.0
            TCP_BACKEND_PORT=58609@127.0.0.1 HTTP_BACKEND_PORT=58611@127.0.0.1@noreuse
            GW_TCP_CIDR_PORT=58612@0.0.0.0 CTRL_METRICS_PORT=58613@127.0.0.1
            UPDATE_SERVER_PORT=58614@0.0.0.0@noreuse
            )
            ;;
        unknown_config_fields)
            ports=(
            CONTROLLER_ADMIN_PORT=58700@127.0.0.1 CONTROLLER_GRPC_PORT=58701@127.0.0.1
            CONTROLLER_PROBE_PORT=58702@127.0.0.1 CONTROLLER_METRICS_PORT=58703@127.0.0.1
            )
            ;;
        proxy_protocol_policy_lifecycle)
            ports=(
            CTRL_ADMIN_PORT=58920@127.0.0.1 CTRL_GRPC_PORT=58921@127.0.0.1 CTRL_PROBE_PORT=58922@127.0.0.1
            GW_ADMIN_PORT=58923@127.0.0.1 GW_PROBE_PORT=58924@127.0.0.1 GW_METRICS_PORT=58925@127.0.0.1
            CTRL_METRICS_PORT=58926@127.0.0.1 LISTENER_PORT=58930@0.0.0.0 TEST_SERVER_PORT=30063@0.0.0.0
            )
            ;;
        proxy_protocol_inbound)
            ports=(
            CTRL_ADMIN_PORT=58420@127.0.0.1 CTRL_GRPC_PORT=58421@127.0.0.1 CTRL_PROBE_PORT=58422@127.0.0.1
            GW_ADMIN_PORT=58423@127.0.0.1 GW_PROBE_PORT=58424@127.0.0.1 GW_METRICS_PORT=58425@127.0.0.1
            CTRL_METRICS_PORT=58426@127.0.0.1 GW_PP_PORT=58430@0.0.0.0 GW_PLAIN_PORT=58431@0.0.0.0
            GW_TRUST_PORT=58432@0.0.0.0 LB_V2_PORT=58440@docker LB_V1_PORT=58441@docker
            LB_V2_PROBE_PORT=58442@docker LB_V1_PROBE_PORT=58443@docker
            V2_DUMP_PORT=58444@0.0.0.0 V1_DUMP_PORT=58445@0.0.0.0 TS_HTTP_PORT=30061@0.0.0.0
            )
            ;;
        proxy_protocol_chain)
            ports=(
            CTRL_ADMIN_PORT=58520@127.0.0.1 CTRL_GRPC_PORT=58521@127.0.0.1 CTRL_PROBE_PORT=58522@127.0.0.1
            GW_ADMIN_PORT=58523@127.0.0.1 GW_PROBE_PORT=58524@127.0.0.1 GW_METRICS_PORT=58525@127.0.0.1
            CTRL_METRICS_PORT=58526@127.0.0.1 GW_PP_PORT=58530@0.0.0.0 GW_HOP2_PORT=58531@0.0.0.0
            LB_V2_PORT=58540@docker LB_V2_PROBE_PORT=58542@docker V2_DUMP_PORT=58544@0.0.0.0
            TS_HTTP_PORT=30062@0.0.0.0
            )
            ;;
        metrics_lifetime)
            ports=(
            CTRL_ADMIN_PORT=59000@127.0.0.1 CTRL_GRPC_PORT=59001@127.0.0.1 CTRL_PROBE_PORT=59002@127.0.0.1
            CTRL_METRICS_PORT=59003@127.0.0.1 GW_ADMIN_PORT=59004@127.0.0.1 GW_PROBE_PORT=59005@127.0.0.1
            GW_METRICS_PORT=59006@127.0.0.1 GW_HTTP_PORT=31149@0.0.0.0 BACKEND_HTTP_PORT=30001@127.0.0.1
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
