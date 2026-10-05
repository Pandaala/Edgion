#!/usr/bin/env bash
# Isolated B0084 LinkSys outbound TLS policy and Etcd reconnect integration test.
#
# This is a standalone-topology exception to the shared integration runner rule:
# it uses dynamic loopback ports, unique Docker objects, and PID-specific cleanup.
# Existing LinkSys runners call broad pkill and must not be invoked by this script.
# Global outbound TLS mutation cannot run in the shared Controller/Gateway topology.
# Runtime fixtures and logs stay under one private tmp/integration_testing directory.
#
# Usage: test_outbound_identity_policy.sh [--no-build] [--preflight]

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/../../../../.." && pwd)"
# shellcheck source=../../utils/conf_sync_certs.sh
source "${SCRIPT_DIR}/../../utils/conf_sync_certs.sh"

DO_BUILD=true
PREFLIGHT_ONLY=false
for arg in "$@"; do
    case "$arg" in
        --no-build) DO_BUILD=false ;;
        --preflight) PREFLIGHT_ONLY=true ;;
        -h|--help)
            sed -n '2,9p' "$0"
            exit 0
            ;;
        *) echo "Unknown argument: $arg" >&2; exit 2 ;;
    esac
done

fail() { echo "FAIL: $*" >&2; exit 1; }
note() { echo "[outbound-identity] $*"; }

for tool in bash python3 jq curl openssl docker base64; do
    command -v "$tool" >/dev/null || fail "Required command is missing: $tool"
done
docker info >/dev/null 2>&1 || fail "Docker daemon is unavailable"
if "$PREFLIGHT_ONLY"; then
    note "Required commands and Docker daemon are available"
    exit 0
fi

if "$DO_BUILD"; then
    (cd "$PROJECT_ROOT" && cargo build --bin edgion-controller --bin edgion-gateway --bin edgion-cli)
fi
for bin in edgion-controller edgion-gateway edgion-cli; do
    test -x "${PROJECT_ROOT}/target/debug/${bin}" || fail "Missing binary: target/debug/${bin}"
done

mkdir -p "${PROJECT_ROOT}/tmp/integration_testing"
WORK_DIR="$(mktemp -d "${PROJECT_ROOT}/tmp/integration_testing/outbound_identity_XXXXXXXX")"
chmod 700 "$WORK_DIR"
mkdir -p "${WORK_DIR}/logs" "${WORK_DIR}/config" "${WORK_DIR}/certs/upstream"
CONTROLLER_PID=""
GATEWAY_PID=""
ETCD_ID=""
REDIS_ID=""
ES_ID=""
NGINX_ID=""
ETCD_PAUSED=false
RUN_ID="${WORK_DIR##*/}"
OWNER_LABEL="edgion.integration.run=${RUN_ID}"
NETWORK_NAME="edgion-${RUN_ID}"
ETCD_CONTAINER="${NETWORK_NAME}-etcd"
REDIS_CONTAINER="${NETWORK_NAME}-redis"
ES_CONTAINER="${NETWORK_NAME}-es"
NGINX_CONTAINER="${NETWORK_NAME}-nginx"
COUNTER_PIDS=()
COUNTER_PID_COUNT=0
owned_container_exists() {
    local label
    label="$(docker inspect --format '{{index .Config.Labels "edgion.integration.run"}}' "$1" 2>/dev/null)" || return 1
    test "$label" = "$RUN_ID"
}
owned_network_exists() {
    local label
    label="$(docker network inspect --format '{{index .Labels "edgion.integration.run"}}' "$NETWORK_NAME" 2>/dev/null)" || return 1
    test "$label" = "$RUN_ID"
}
stop_owned_pid() {
    local pid="$1" state second
    test -n "$pid" || return 0
    kill -TERM "$pid" >/dev/null 2>&1 || true
    for ((second=0; second<5; second=second+1)); do
        state="$(ps -o stat= -p "$pid" 2>/dev/null | tr -d ' ' || true)"
        case "$state" in
            ''|Z*) wait "$pid" >/dev/null 2>&1 || true; return 0 ;;
        esac
        sleep 1
    done
    kill -KILL "$pid" >/dev/null 2>&1 || true
    wait "$pid" >/dev/null 2>&1 || true
}
cleanup() {
    local status=$?
    trap - EXIT
    if "$ETCD_PAUSED" && test -n "$ETCD_ID"; then
        docker unpause "$ETCD_ID" >/dev/null 2>&1 || true
    fi
    local pid
    for pid in "$GATEWAY_PID" "$CONTROLLER_PID"; do
        stop_owned_pid "$pid"
    done
    if test "$COUNTER_PID_COUNT" -gt 0; then
        for pid in "${COUNTER_PIDS[@]}"; do
            stop_owned_pid "$pid"
        done
    fi
    local container
    for container in "$NGINX_CONTAINER" "$ES_CONTAINER" "$REDIS_CONTAINER" "$ETCD_CONTAINER"; do
        if owned_container_exists "$container"; then
            docker rm -f "$container" >/dev/null 2>&1 || true
        fi
    done
    # The daemon may create an object and return EOF before reporting its ID.
    if owned_network_exists; then docker network rm "$NETWORK_NAME" >/dev/null 2>&1 || true; fi
    note "Evidence directory: $WORK_DIR"
    exit "$status"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

# Hold all sockets while choosing ports so this run cannot choose duplicates.
# A later bind failure is a hard failure, never a reason to use another
# worktree's listener.
PORT_LIST="$(python3 -c 'import socket; ss = [socket.socket() for _ in range(8)]; [s.bind(("127.0.0.1", 0)) for s in ss]; print(" ".join(str(s.getsockname()[1]) for s in ss)); [s.close() for s in ss]')"
read -r CTRL_GRPC_PORT CTRL_ADMIN_PORT CTRL_PROBE_PORT CTRL_METRICS_PORT \
    GW_ADMIN_PORT GW_PROBE_PORT GW_METRICS_PORT GW_LISTENER_PORT <<< "$PORT_LIST"
CTRL_URL="http://127.0.0.1:${CTRL_ADMIN_PORT}"
GW_URL="http://127.0.0.1:${GW_ADMIN_PORT}"
export EDGION_ADMIN_TOKEN="${EDGION_ADMIN_TOKEN:-integration-controller-admin-token-0123456789}"
export EDGION_TOKEN="$EDGION_ADMIN_TOKEN"
export EDGION_GATEWAY_ADMIN_TOKEN="${EDGION_GATEWAY_ADMIN_TOKEN:-integration-gateway-admin-token-0123456789}"
export EDGION_WORK_DIR="$WORK_DIR"

wait_for() {
    local label="$1" timeout="$2"
    shift 2
    local second
    for ((second=0; second<timeout; second=second+1)); do
        if "$@"; then return 0; fi
        sleep 1
    done
    fail "Timed out waiting for $label"
}
probe_ready() { curl -fsS --max-time 2 "$1/ready" >/dev/null 2>&1; }
gw_api() {
    local method="$1" path="$2"
    shift 2
    curl -fsS --max-time 12 -X "$method" "${GW_URL}${path}" \
        -H "Authorization: Bearer ${EDGION_GATEWAY_ADMIN_TOKEN}" \
        -H 'Content-Type: application/json' "$@"
}
api_ok() { gw_api GET "$1" 2>/dev/null | jq -e '.success == true' >/dev/null 2>&1; }
apply_file() { "${PROJECT_ROOT}/target/debug/edgion-cli" --server "$CTRL_URL" apply -f "$1" >/dev/null; }
docker_host_port() {
    local mapping
    mapping="$(docker port "$1" "$2/tcp")"
    printf '%s\n' "$mapping" | awk -F: 'END {print $NF}'
}

conf_sync_distribute_certs "$WORK_DIR" >/dev/null
cp -R "${PROJECT_ROOT}/config/crd" "${WORK_DIR}/config/crd"
cp "${PROJECT_ROOT}/config/edgion-gateway.yaml" "${WORK_DIR}/gateway.yaml"
cat >> "${WORK_DIR}/gateway.yaml" <<EOF

metrics:
  listen: "127.0.0.1:${GW_METRICS_PORT}"
EOF

CERT_DIR="${WORK_DIR}/certs/upstream"
openssl req -x509 -newkey rsa:2048 -nodes -days 2 \
    -subj '/CN=outbound-identity-test-ca' \
    -keyout "${CERT_DIR}/ca.key" -out "${CERT_DIR}/ca.crt" >/dev/null 2>&1
openssl req -newkey rsa:2048 -nodes -subj '/CN=localhost' \
    -keyout "${CERT_DIR}/server.key" -out "${CERT_DIR}/server.csr" >/dev/null 2>&1
cat > "${CERT_DIR}/server.ext" <<'EOF'
basicConstraints=CA:FALSE
keyUsage=digitalSignature,keyEncipherment
extendedKeyUsage=serverAuth
subjectAltName=DNS:localhost,IP:127.0.0.1
EOF
openssl x509 -req -days 2 -in "${CERT_DIR}/server.csr" \
    -CA "${CERT_DIR}/ca.crt" -CAkey "${CERT_DIR}/ca.key" -CAcreateserial \
    -extfile "${CERT_DIR}/server.ext" -out "${CERT_DIR}/server.crt" >/dev/null 2>&1
chmod 644 "${CERT_DIR}/server.key" "${CERT_DIR}/server.crt" "${CERT_DIR}/ca.crt"
openssl verify -CAfile "${CERT_DIR}/ca.crt" "${CERT_DIR}/server.crt" >/dev/null

docker network create --label "$OWNER_LABEL" "$NETWORK_NAME" >/dev/null
# Etcd serves a certificate signed by this run's CA. --trusted-ca-file
# enables client-certificate authentication, which this server-auth test omits.
ETCD_ID="$(docker run -d --name "$ETCD_CONTAINER" --label "$OWNER_LABEL" \
    --network "$NETWORK_NAME" --network-alias etcd \
    -p '127.0.0.1::2379' -v "${CERT_DIR}:/certs:ro" quay.io/coreos/etcd:v3.5.17 \
    /usr/local/bin/etcd --name outbound-identity-etcd --data-dir /etcd-data \
    --listen-client-urls https://0.0.0.0:2379 \
    --advertise-client-urls https://127.0.0.1:2379 \
    --listen-peer-urls http://127.0.0.1:2380 \
    --initial-advertise-peer-urls http://127.0.0.1:2380 \
    --initial-cluster outbound-identity-etcd=http://127.0.0.1:2380 \
    --cert-file /certs/server.crt --key-file /certs/server.key)"
REDIS_ID="$(docker run -d --name "$REDIS_CONTAINER" --label "$OWNER_LABEL" \
    --network "$NETWORK_NAME" \
    -p '127.0.0.1::6380' -v "${CERT_DIR}:/certs:ro" redis:7-alpine \
    redis-server --port 0 --tls-port 6380 \
    --tls-cert-file /certs/server.crt --tls-key-file /certs/server.key \
    --tls-ca-cert-file /certs/ca.crt --tls-auth-clients no)"
ES_ID="$(docker run -d --name "$ES_CONTAINER" --label "$OWNER_LABEL" \
    --network "$NETWORK_NAME" --network-alias es \
    -e discovery.type=single-node -e xpack.security.enabled=false \
    -e xpack.security.http.ssl.enabled=false -e ES_JAVA_OPTS='-Xms256m -Xmx256m' \
    docker.elastic.co/elasticsearch/elasticsearch:8.17.0)"
cat > "${WORK_DIR}/nginx.conf" <<'EOF'
events {}
http {
    server {
        listen 443 ssl;
        ssl_certificate /certs/server.crt;
        ssl_certificate_key /certs/server.key;
        location / { proxy_pass http://es:9200; }
    }
}
EOF
NGINX_ID="$(docker run -d --name "$NGINX_CONTAINER" --label "$OWNER_LABEL" \
    --network "$NETWORK_NAME" \
    -p '127.0.0.1::443' -v "${CERT_DIR}:/certs:ro" \
    -v "${WORK_DIR}/nginx.conf:/etc/nginx/nginx.conf:ro" nginx:1.27-alpine)"
ETCD_PORT="$(docker_host_port "$ETCD_ID" 2379)"
REDIS_PORT="$(docker_host_port "$REDIS_ID" 6380)"
ES_PORT="$(docker_host_port "$NGINX_ID" 443)"
test -n "$ETCD_PORT" && test -n "$REDIS_PORT" && test -n "$ES_PORT" || fail "Docker port allocation failed"
etcd_ready() { docker exec "$ETCD_ID" etcdctl --endpoints=https://127.0.0.1:2379 --cacert=/certs/ca.crt endpoint health >/dev/null 2>&1; }
redis_ready() { docker exec "$REDIS_ID" redis-cli --tls --cacert /certs/ca.crt -p 6380 ping 2>/dev/null | grep -qx PONG; }
es_ready() { docker exec "$ES_ID" curl -fsS http://127.0.0.1:9200/_cluster/health >/dev/null 2>&1; }
nginx_ready() { curl -fsS --max-time 2 --cacert "${CERT_DIR}/ca.crt" "https://127.0.0.1:${ES_PORT}/_cluster/health" >/dev/null 2>&1; }
wait_for 'Etcd TLS' 60 etcd_ready
wait_for 'Redis TLS' 60 redis_ready
wait_for 'Elasticsearch' 120 es_ready
wait_for 'Elasticsearch HTTPS proxy' 30 nginx_ready
note 'Owned TLS services are ready'

"${PROJECT_ROOT}/target/debug/edgion-controller" \
    -c "${PROJECT_ROOT}/config/edgion-controller.yaml" \
    --work-dir "$WORK_DIR" --conf-dir "${WORK_DIR}/config" --test-mode \
    --grpc-listen "127.0.0.1:${CTRL_GRPC_PORT}" \
    --admin-listen "127.0.0.1:${CTRL_ADMIN_PORT}" \
    --probe-listen "127.0.0.1:${CTRL_PROBE_PORT}" \
    --metrics-listen "127.0.0.1:${CTRL_METRICS_PORT}" \
    > "${WORK_DIR}/logs/controller.log" 2>&1 &
CONTROLLER_PID=$!
wait_for 'Controller' 45 probe_ready "http://127.0.0.1:${CTRL_PROBE_PORT}"

apply_file "${PROJECT_ROOT}/edgion-tests/integration/conf/base/GatewayClass.yaml"
cat > "${WORK_DIR}/Gateway_edgion-test_outbound-identity.yaml" <<EOF
apiVersion: gateway.networking.k8s.io/v1
kind: Gateway
metadata:
  name: outbound-identity
  namespace: edgion-test
spec:
  gatewayClassName: public-gateway
  listeners:
    - name: http
      protocol: HTTP
      port: ${GW_LISTENER_PORT}
EOF
apply_file "${WORK_DIR}/Gateway_edgion-test_outbound-identity.yaml"

CA_B64="$(base64 < "${CERT_DIR}/ca.crt" | tr -d '\n')"
for namespace in default edgion-test; do
    cat > "${WORK_DIR}/Secret_${namespace}_outbound-identity-ca.yaml" <<EOF
apiVersion: v1
kind: Secret
metadata:
  name: outbound-identity-ca
  namespace: ${namespace}
type: Opaque
data:
  ca.crt: ${CA_B64}
EOF
    apply_file "${WORK_DIR}/Secret_${namespace}_outbound-identity-ca.yaml"
done
cat > "${WORK_DIR}/ReferenceGrant_edgion-test_outbound-identity-ca.yaml" <<'EOF'
apiVersion: gateway.networking.k8s.io/v1
kind: ReferenceGrant
metadata:
  name: outbound-identity-ca
  namespace: edgion-test
spec:
  from:
    - group: edgion.io
      kind: EdgionGatewayConfig
      namespace: default
  to:
    - group: ""
      kind: Secret
      name: outbound-identity-ca
EOF
apply_file "${WORK_DIR}/ReferenceGrant_edgion-test_outbound-identity-ca.yaml"

write_egc() {
    cp "${PROJECT_ROOT}/edgion-tests/integration/conf/base/EdgionGatewayConfig.yaml" \
        "${WORK_DIR}/EdgionGatewayConfig_example-gateway.yaml"
    case "$1" in
        hostname)
            cat >> "${WORK_DIR}/EdgionGatewayConfig_example-gateway.yaml" <<'EOF'
  outboundTls:
    validation:
      wellKnownCACertificates: System
      hostname: unsupported.example.com
EOF
            ;;
        san)
            cat >> "${WORK_DIR}/EdgionGatewayConfig_example-gateway.yaml" <<'EOF'
  outboundTls:
    validation:
      wellKnownCACertificates: System
      subjectAltNames:
        - type: Hostname
          hostname: unsupported.example.com
EOF
            ;;
        ca)
            cat >> "${WORK_DIR}/EdgionGatewayConfig_example-gateway.yaml" <<'EOF'
  outboundTls:
    validation:
      caCertificateRefs:
        - kind: Secret
          name: outbound-identity-ca
          namespace: edgion-test
EOF
            ;;
        *) fail "Unknown EGC policy: $1" ;;
    esac
    apply_file "${WORK_DIR}/EdgionGatewayConfig_example-gateway.yaml"
}
egc_policy_published() {
    local expected="$1"
    gw_api GET '/configclient/EdgionGatewayConfig?name=example-gateway' 2>/dev/null | \
        jq -e --arg expected "$expected" '
            (.success == true) and
            ((.data.spec.outboundTls.validation) as $validation |
                if $expected == "hostname" then
                    $validation == {
                        wellKnownCACertificates: "System",
                        hostname: "unsupported.example.com"
                    }
                elif $expected == "san" then
                    $validation == {
                        wellKnownCACertificates: "System",
                        subjectAltNames: [{type: "Hostname", hostname: "unsupported.example.com"}]
                    }
                else
                    ($validation | keys) == ["caCertificateRefs"] and
                    ($validation.caCertificateRefs | length) == 1 and
                    $validation.caCertificateRefs[0].kind == "Secret" and
                    $validation.caCertificateRefs[0].name == "outbound-identity-ca" and
                    $validation.caCertificateRefs[0].namespace == "edgion-test"
                end)
        ' >/dev/null 2>&1
}
write_egc hostname

EDGION_ACCESS_LOG="${WORK_DIR}/logs/access.log" \
EDGION_TEST_ACCESS_LOG_PATH="${WORK_DIR}/logs/access.log" \
EDGION_ALLOW_INTEGRATION_TESTING_MODE=1 \
"${PROJECT_ROOT}/target/debug/edgion-gateway" \
    -c "${WORK_DIR}/gateway.yaml" --work-dir "$WORK_DIR" \
    --server-addr "https://127.0.0.1:${CTRL_GRPC_PORT}" \
    --admin-listen "127.0.0.1:${GW_ADMIN_PORT}" \
    --probe-listen "127.0.0.1:${GW_PROBE_PORT}" --bind-ip 127.0.0.1 \
    --integration-testing-mode > "${WORK_DIR}/logs/gateway.log" 2>&1 &
GATEWAY_PID=$!
wait_for 'Gateway' 90 probe_ready "http://127.0.0.1:${GW_PROBE_PORT}"
wait_for 'effective hostname EGC publication' 45 egc_policy_published hostname

# A dedicated listener records every accepted socket, then closes it. Its own
# port is kernel-assigned and used only for the negative no-dial assertion.
cat > "${WORK_DIR}/accept_counter.py" <<'PY'
import socket
import sys

listener = socket.socket()
listener.bind(("127.0.0.1", 0))
listener.listen(16)
with open(sys.argv[1], "w", encoding="ascii") as port_file:
    port_file.write(str(listener.getsockname()[1]))
while True:
    connection, _ = listener.accept()
    with open(sys.argv[2], "a", encoding="ascii") as count_file:
        count_file.write("accept\n")
    connection.close()
PY
for provider in redis etcd es; do
    python3 "${WORK_DIR}/accept_counter.py" \
        "${WORK_DIR}/${provider}.port" "${WORK_DIR}/${provider}.accepts" \
        > "${WORK_DIR}/logs/${provider}-counter.log" 2>&1 &
    COUNTER_PIDS+=("$!")
    COUNTER_PID_COUNT=$((COUNTER_PID_COUNT + 1))
done
counters_ready() {
    test -s "${WORK_DIR}/redis.port" && test -s "${WORK_DIR}/etcd.port" && test -s "${WORK_DIR}/es.port"
}
wait_for 'TCP accept counters' 10 counters_ready

write_linksys() {
    local provider="$1" name="$2" endpoint="$3" local_ca="$4"
    local file="${WORK_DIR}/LinkSys_default_${name}.yaml"
    cat > "$file" <<EOF
apiVersion: edgion.io/v1
kind: LinkSys
metadata:
  name: ${name}
  namespace: default
spec:
  type: ${provider}
  config:
    endpoints:
      - "${endpoint}"
EOF
    if test "$provider" = redis; then printf '    db: 0\n' >> "$file"; fi
    cat >> "$file" <<'EOF'
    tls:
      enabled: true
EOF
    if test "$local_ca" = yes; then
        cat >> "$file" <<'EOF'
      validation:
        caCertificateRefs:
          - kind: Secret
            name: outbound-identity-ca
EOF
    fi
    case "$provider" in
        redis) printf '    pool:\n      size: 1\n    timeout:\n      connect: 3s\n      command: 2s\n' >> "$file" ;;
        etcd) printf '    timeout:\n      dial: 5000\n      request: 5000\n' >> "$file" ;;
        elasticsearch) printf '    timeout:\n      connect: 5000\n      request: 10000\n' >> "$file" ;;
    esac
    apply_file "$file"
}
provider_path() {
    case "$1" in elasticsearch) echo es ;; *) echo "$1" ;; esac
}
resource_synced() { api_ok "/configclient/LinkSys?namespace=default&name=$1"; }
rejection_recorded() {
    grep -F "$1" "${WORK_DIR}/logs/gateway.log" 2>/dev/null | \
        grep -F 'hostname/subjectAltNames policy is unsupported' >/dev/null
}
assert_no_dial() {
    local name="$1" provider="$2" counter="$3" path
    wait_for "effective rejection of $name" 45 rejection_recorded "$name"
    wait_for "Gateway publication of $name" 30 resource_synced "$name"
    path="$(provider_path "$provider")"
    if api_ok "/api/v1/testing/link-sys/${path}/default_${name}/ping"; then
        fail "Rejected client $name completed a live operation"
    fi
    if test -s "${WORK_DIR}/${counter}.accepts"; then
        fail "Rejected $name dialed its endpoint"
    fi
}

for provider in redis etcd elasticsearch; do
    counter="$(provider_path "$provider")"
    port="$(cat "${WORK_DIR}/${counter}.port")"
    case "$provider" in redis) scheme=rediss ;; *) scheme=https ;; esac
    name="policy-${counter}-hostname"
    write_linksys "$provider" "$name" "${scheme}://127.0.0.1:${port}" no
    assert_no_dial "$name" "$provider" "$counter"
done
note 'Inherited hostname rejection has zero endpoint dials'

write_egc san
wait_for 'effective SAN EGC publication' 45 egc_policy_published san
for provider in redis etcd elasticsearch; do
    counter="$(provider_path "$provider")"
    port="$(cat "${WORK_DIR}/${counter}.port")"
    case "$provider" in redis) scheme=rediss ;; *) scheme=https ;; esac
    name="policy-${counter}-san"
    write_linksys "$provider" "$name" "${scheme}://127.0.0.1:${port}" no
    assert_no_dial "$name" "$provider" "$counter"
done
sleep 2
for counter in redis etcd es; do
    test ! -s "${WORK_DIR}/${counter}.accepts" || fail "A rejected policy dialed the $counter counter"
done
note 'Inherited SAN rejection has zero endpoint dials'

# Local validation is a whole-block override. It shadows the unsupported
# inherited SAN and trusts this run's private CA in the LinkSys namespace.
write_linksys redis policy-redis-local "rediss://127.0.0.1:${REDIS_PORT}" yes
wait_for 'local override Redis TLS handshake' 60 api_ok '/api/v1/testing/link-sys/redis/default_policy-redis-local/ping'
note 'Local CA override completed a real Redis TLS handshake'

# These three resources remain unchanged across the EGC update. Their later
# successful operations prove EGC publication rebuilt provider clients.
write_linksys redis policy-redis-recovery "rediss://127.0.0.1:${REDIS_PORT}" no
write_linksys etcd policy-etcd-recovery "https://127.0.0.1:${ETCD_PORT}" no
write_linksys elasticsearch policy-es-recovery "https://127.0.0.1:${ES_PORT}" no
for provider in redis etcd es; do
    name="policy-${provider}-recovery"
    wait_for "pre-recovery rejection of $name" 45 rejection_recorded "$name"
done
write_egc ca
wait_for 'effective CA EGC publication' 45 egc_policy_published ca
wait_for 'global CA Redis TLS handshake' 90 api_ok '/api/v1/testing/link-sys/redis/default_policy-redis-recovery/ping'
wait_for 'global CA Etcd TLS handshake' 90 api_ok '/api/v1/testing/link-sys/etcd/default_policy-etcd-recovery/ping'
wait_for 'global CA Elasticsearch TLS handshake' 120 api_ok '/api/v1/testing/link-sys/es/default_policy-es-recovery/ping'
put_path='/api/v1/testing/link-sys/etcd/default_policy-etcd-recovery/put'
get_path='/api/v1/testing/link-sys/etcd/default_policy-etcd-recovery/get/outbound-identity-test'
put_body='{"key":"outbound-identity-test","value":"before-pause"}'
gw_api POST "$put_path" -d "$put_body" | jq -e '.success == true' >/dev/null || fail 'Etcd TLS PUT failed'
gw_api GET "$get_path" | jq -e '.success == true and .data.value == "before-pause"' >/dev/null || fail 'Etcd TLS GET failed'
note 'All three inherited private-CA TLS clients recovered without a binary restart'

gateway_log_bytes() { wc -c < "${WORK_DIR}/logs/gateway.log" | tr -d '[:space:]'; }
etcd_monitor_pair_since() {
    python3 - "${WORK_DIR}/logs/gateway.log" "$1" <<'PY'
import re
import sys

with open(sys.argv[1], "rb") as log_file:
    offset = int(sys.argv[2])
    if offset:
        log_file.seek(offset - 1)
        started_mid_line = log_file.read(1) != b"\n"
    else:
        started_mid_line = False
    log_file.seek(offset)
    lines = log_file.read().decode("utf-8", errors="replace").splitlines()
    if started_mid_line:
        lines = lines[1:]

strip_color = re.compile(r"\x1b\[[0-9;]*m")
target = re.compile(r"(?:^|\s)etcd=default/policy-etcd-recovery(?:\s|$)")
unhealthy_seen = False
for raw_line in lines:
    line = strip_color.sub("", raw_line)
    if not target.search(line):
        continue
    if "Etcd unhealthy, attempting reconnect" in line:
        unhealthy_seen = True
    elif unhealthy_seen and "Etcd reconnected successfully" in line:
        sys.exit(0)
sys.exit(1)
PY
}
before_pause_log_bytes="$(gateway_log_bytes)"
docker pause "$ETCD_ID" >/dev/null
ETCD_PAUSED=true
etcd_probe_failed() {
    probe_ready "http://127.0.0.1:${GW_PROBE_PORT}" &&
        egc_policy_published ca &&
        ! api_ok '/api/v1/testing/link-sys/etcd/default_policy-etcd-recovery/ping'
}
wait_for 'Etcd probe failure while paused' 35 etcd_probe_failed
wait_for 'Etcd monitor handled the paused service' 70 etcd_monitor_pair_since "$before_pause_log_bytes"
docker unpause "$ETCD_ID" >/dev/null
ETCD_PAUSED=false
wait_for 'Etcd transport after unpause' 120 api_ok '/api/v1/testing/link-sys/etcd/default_policy-etcd-recovery/ping'
gw_api GET "$get_path" | jq -e '.success == true and .data.value == "before-pause"' >/dev/null || fail 'Etcd value missing after reconnect'
gw_api POST "$put_path" -d '{"key":"outbound-identity-test","value":"after-pause"}' | \
    jq -e '.success == true' >/dev/null || fail 'Etcd write failed after reconnect'
gw_api GET "$get_path" | jq -e '.success == true and .data.value == "after-pause"' >/dev/null || fail 'Etcd new value missing after reconnect'
note 'Etcd TLS monitor reconnected and read/write operations passed'
