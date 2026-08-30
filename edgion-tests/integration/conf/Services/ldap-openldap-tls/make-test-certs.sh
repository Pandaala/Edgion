#!/usr/bin/env bash
# Generate the self-contained LDAP TLS certificate set used by integration tests.
#
# The certs directory is intentionally gitignored. This script is therefore run
# automatically before the Docker service starts. When
# EDGION_GENERATED_SECRET_DIR is set, it also writes matching runtime Secret
# manifests so the gateway trusts the exact CA generated for the current checkout.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CERTS_DIR="${SCRIPT_DIR}/certs"

cert_set_is_valid() {
    local required=(
        ca.crt
        server.crt
        server.key
        client.crt
        client.key
        wrong-ca.crt
    )
    local name
    for name in "${required[@]}"; do
        if [ ! -f "${CERTS_DIR}/${name}" ]; then
            return 1
        fi
    done

    openssl verify -CAfile "${CERTS_DIR}/ca.crt" \
        "${CERTS_DIR}/server.crt" "${CERTS_DIR}/client.crt" >/dev/null 2>&1 &&
        openssl x509 -in "${CERTS_DIR}/wrong-ca.crt" -noout >/dev/null 2>&1
}

remove_docker_mount_directories() {
    local name
    for name in ca.crt server.crt server.key; do
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
        -subj "/CN=edgion-test-ldap-ca" >/dev/null 2>&1

    openssl req -newkey rsa:2048 \
        -keyout "${temp_dir}/server.key" \
        -out "${temp_dir}/server.csr" \
        -nodes \
        -subj "/CN=edgion-test-openldap-tls" >/dev/null 2>&1
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
            'subjectAltName=DNS:edgion-test-openldap-tls,IP:127.0.0.1') \
        >/dev/null 2>&1

    openssl req -newkey rsa:2048 \
        -keyout "${temp_dir}/client.key" \
        -out "${temp_dir}/client.csr" \
        -nodes \
        -subj "/CN=edgion-test-ldap-client" >/dev/null 2>&1
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

    openssl req -x509 -newkey rsa:2048 \
        -keyout "${temp_dir}/wrong-ca.key" \
        -out "${temp_dir}/wrong-ca.crt" \
        -days 3650 -nodes \
        -subj "/CN=edgion-test-wrong-ca" >/dev/null 2>&1

    cp "${temp_dir}/ca.crt" "${CERTS_DIR}/ca.crt"
    cp "${temp_dir}/server.crt" "${CERTS_DIR}/server.crt"
    cp "${temp_dir}/server.key" "${CERTS_DIR}/server.key"
    cp "${temp_dir}/client.crt" "${CERTS_DIR}/client.crt"
    cp "${temp_dir}/client.key" "${CERTS_DIR}/client.key"
    cp "${temp_dir}/wrong-ca.crt" "${CERTS_DIR}/wrong-ca.crt"
    chmod 644 "${CERTS_DIR}/ca.crt" "${CERTS_DIR}/server.crt" \
        "${CERTS_DIR}/client.crt" "${CERTS_DIR}/wrong-ca.crt"
    chmod 600 "${CERTS_DIR}/server.key" "${CERTS_DIR}/client.key"
}

write_runtime_secrets() {
    if [ -z "${EDGION_GENERATED_SECRET_DIR:-}" ]; then
        return 0
    fi

    local output_dir="${EDGION_GENERATED_SECRET_DIR}/EdgionPlugins/LdapAuthTls"
    local ca_b64
    local client_cert_b64
    local client_key_b64
    local wrong_ca_b64
    mkdir -p "$output_dir"
    ca_b64=$(base64 < "${CERTS_DIR}/ca.crt" | tr -d '\n')
    client_cert_b64=$(base64 < "${CERTS_DIR}/client.crt" | tr -d '\n')
    client_key_b64=$(base64 < "${CERTS_DIR}/client.key" | tr -d '\n')
    wrong_ca_b64=$(base64 < "${CERTS_DIR}/wrong-ca.crt" | tr -d '\n')

    cat > "${output_dir}/Secret_ldap-ca-bundle.yaml" <<EOF
apiVersion: v1
kind: Secret
metadata:
  name: ldap-ca-bundle
  namespace: edgion-default
type: Opaque
data:
  ca.crt: ${ca_b64}
EOF

    cat > "${output_dir}/Secret_ldap-client-identity.yaml" <<EOF
apiVersion: v1
kind: Secret
metadata:
  name: ldap-client-identity
  namespace: edgion-default
type: kubernetes.io/tls
data:
  tls.crt: ${client_cert_b64}
  tls.key: ${client_key_b64}
EOF

    cat > "${output_dir}/Secret_ldap-wrong-ca-bundle.yaml" <<EOF
apiVersion: v1
kind: Secret
metadata:
  name: ldap-wrong-ca-bundle
  namespace: edgion-default
type: Opaque
data:
  ca.crt: ${wrong_ca_b64}
EOF
}

if ! cert_set_is_valid; then
    generate_cert_set
fi
write_runtime_secrets
