#!/usr/bin/env bash
# Generate the Redis TLS certificate set used by integration tests.
#
# The certs directory is gitignored. Run this script before Docker Compose so
# Redis and the generated LinkSys Secrets always use the same ephemeral CA.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CERTS_DIR="${SCRIPT_DIR}/certs"

cert_set_is_valid() {
    local required=(ca.crt server.crt server.key client.crt client.key)
    local name
    for name in "${required[@]}"; do
        if [ ! -f "${CERTS_DIR}/${name}" ]; then
            return 1
        fi
    done

    openssl verify -CAfile "${CERTS_DIR}/ca.crt" \
        "${CERTS_DIR}/server.crt" "${CERTS_DIR}/client.crt" >/dev/null 2>&1
}

remove_docker_mount_directories() {
    local name
    for name in ca.crt server.crt server.key client.crt client.key; do
        local path="${CERTS_DIR}/${name}"
        if [ -d "$path" ]; then
            rmdir "$path"
        fi
    done
}

generate_cert_set() {
    mkdir -p "$CERTS_DIR"
    remove_docker_mount_directories

    local temp_dir
    temp_dir=$(mktemp -d)
    trap 'rm -rf "$temp_dir"' RETURN

    openssl req -x509 -newkey rsa:2048 \
        -keyout "${temp_dir}/ca.key" \
        -out "${temp_dir}/ca.crt" \
        -days 3650 -nodes \
        -subj "/CN=edgion-test-redis-ca" >/dev/null 2>&1

    openssl req -newkey rsa:2048 \
        -keyout "${temp_dir}/server.key" \
        -out "${temp_dir}/server.csr" \
        -nodes \
        -subj "/CN=edgion-test-redis-tls" >/dev/null 2>&1
    openssl x509 -req \
        -in "${temp_dir}/server.csr" \
        -CA "${temp_dir}/ca.crt" \
        -CAkey "${temp_dir}/ca.key" \
        -CAcreateserial \
        -out "${temp_dir}/server.crt" \
        -days 3650 \
        -extfile <(printf '%s\n' \
            'basicConstraints=CA:FALSE' \
            'keyUsage=digitalSignature,keyEncipherment' \
            'extendedKeyUsage=serverAuth' \
            'subjectAltName=DNS:edgion-test-redis-tls,IP:127.0.0.1') \
        >/dev/null 2>&1

    openssl req -newkey rsa:2048 \
        -keyout "${temp_dir}/client.key" \
        -out "${temp_dir}/client.csr" \
        -nodes \
        -subj "/CN=edgion-test-redis-client" >/dev/null 2>&1
    openssl x509 -req \
        -in "${temp_dir}/client.csr" \
        -CA "${temp_dir}/ca.crt" \
        -CAkey "${temp_dir}/ca.key" \
        -CAcreateserial \
        -out "${temp_dir}/client.crt" \
        -days 3650 \
        -extfile <(printf '%s\n' \
            'basicConstraints=CA:FALSE' \
            'keyUsage=digitalSignature,keyEncipherment' \
            'extendedKeyUsage=clientAuth') \
        >/dev/null 2>&1

    cp "${temp_dir}/ca.crt" "${CERTS_DIR}/ca.crt"
    cp "${temp_dir}/server.crt" "${CERTS_DIR}/server.crt"
    cp "${temp_dir}/server.key" "${CERTS_DIR}/server.key"
    cp "${temp_dir}/client.crt" "${CERTS_DIR}/client.crt"
    cp "${temp_dir}/client.key" "${CERTS_DIR}/client.key"
    chmod 644 "${CERTS_DIR}"/*
}

write_runtime_secrets() {
    if [ -z "${EDGION_GENERATED_SECRET_DIR:-}" ]; then
        return 0
    fi

    local output_dir="${EDGION_GENERATED_SECRET_DIR}/LinkSys/RedisTls"
    local ca_b64 client_cert_b64 client_key_b64
    mkdir -p "$output_dir"
    ca_b64=$(base64 < "${CERTS_DIR}/ca.crt" | tr -d '\n')
    client_cert_b64=$(base64 < "${CERTS_DIR}/client.crt" | tr -d '\n')
    client_key_b64=$(base64 < "${CERTS_DIR}/client.key" | tr -d '\n')

    cat > "${output_dir}/Secret_redis-tls-ca.yaml" <<EOF
apiVersion: v1
kind: Secret
metadata:
  name: redis-tls-ca
  namespace: default
type: Opaque
data:
  ca.crt: ${ca_b64}
EOF

    cat > "${output_dir}/Secret_redis-tls-client.yaml" <<EOF
apiVersion: v1
kind: Secret
metadata:
  name: redis-tls-client
  namespace: default
type: kubernetes.io/tls
data:
  tls.crt: ${client_cert_b64}
  tls.key: ${client_key_b64}
EOF
}

if ! cert_set_is_valid; then
    generate_cert_set
fi
write_runtime_secrets
