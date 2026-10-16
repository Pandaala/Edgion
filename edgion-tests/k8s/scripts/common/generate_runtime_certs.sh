#!/usr/bin/env bash
set -euo pipefail

OUTPUT_DIR="${1:-}"
if [[ -z "${OUTPUT_DIR}" ]]; then
  echo "usage: $0 <run-specific-output-directory>" >&2
  exit 1
fi
SECRETS_DIR="${OUTPUT_DIR}/secrets"
CERTS_DIR="${OUTPUT_DIR}/certs"

for cmd in openssl base64; do
  if ! command -v "${cmd}" >/dev/null 2>&1; then
    echo "${cmd} not found in PATH"
    exit 1
  fi
done

rm -rf "${SECRETS_DIR}" "${CERTS_DIR}"
mkdir -p "${SECRETS_DIR}" "${CERTS_DIR}"

TMP_DIR="$(mktemp -d /tmp/edgion-k8s-certs.XXXXXX)"
cleanup() {
  rm -rf "${TMP_DIR}"
}
trap cleanup EXIT

b64() {
  base64 <"$1" | tr -d '\n'
}

write_tls_secret() {
  local out="$1"
  local name="$2"
  local namespace="$3"
  local crt="$4"
  local key="$5"
  cat >"${out}" <<EOF
apiVersion: v1
kind: Secret
metadata:
  name: ${name}
  namespace: ${namespace}
type: kubernetes.io/tls
data:
  tls.crt: $(b64 "${crt}")
  tls.key: $(b64 "${key}")
EOF
}

write_opaque_ca_secret() {
  local out="$1"
  local name="$2"
  local namespace="$3"
  local ca_file="$4"
  cat >"${out}" <<EOF
apiVersion: v1
kind: Secret
metadata:
  name: ${name}
  namespace: ${namespace}
type: Opaque
data:
  ca.crt: $(b64 "${ca_file}")
EOF
}

write_ca_configmap() {
  local out="$1"
  local name="$2"
  local namespace="$3"
  local ca_file="$4"
  cat >"${out}" <<EOF
apiVersion: v1
kind: ConfigMap
metadata:
  name: ${name}
  namespace: ${namespace}
data:
  ca.crt: |
EOF
  sed 's/^/    /' "${ca_file}" >>"${out}"
}

write_configmap_from_files() {
  local out="$1"
  local name="$2"
  local namespace="$3"
  local dir="$4"
  cat >"${out}" <<EOF
apiVersion: v1
kind: ConfigMap
metadata:
  name: ${name}
  namespace: ${namespace}
EOF

  echo "data:" >>"${out}"
  for file in "${dir}"/*; do
    [[ -f "${file}" ]] || continue
    local key
    key="$(basename "${file}")"
    echo "  ${key}: |" >>"${out}"
    sed 's/^/    /' "${file}" >>"${out}"
  done
}

echo "[certs] generate edge-tls certificate"
openssl req -x509 -newkey rsa:2048 -nodes \
  -keyout "${TMP_DIR}/edge-tls.key" \
  -out "${TMP_DIR}/edge-tls.crt" \
  -days 365 \
  -subj "/CN=test.example.com" \
  -addext "subjectAltName=DNS:test.example.com,DNS:grpc.example.com,DNS:tcp.example.com,DNS:match-test.example.com,DNS:*.wildcard.example.com,DNS:section-test.example.com,DNS:gateway-tls.test.com,DNS:default.example.com,DNS:both-absent-tls.example.com,DNS:*.example.com" \
  >/dev/null 2>&1

openssl ecparam -name prime256v1 -genkey -noout -out "${TMP_DIR}/edge-tls-ecdsa.key" 2>/dev/null
openssl req -x509 -new -nodes \
  -key "${TMP_DIR}/edge-tls-ecdsa.key" \
  -out "${TMP_DIR}/edge-tls-ecdsa.crt" \
  -days 365 \
  -subj "/CN=ecdsa-dualcert.example.com" \
  -addext "subjectAltName=DNS:gateway-tls.test.com,DNS:ecdsa-dualcert.example.com" \
  >/dev/null 2>&1

echo "[certs] generate mTLS CA and server certificates"
openssl req -x509 -newkey rsa:2048 -nodes \
  -keyout "${TMP_DIR}/client-ca.key" \
  -out "${TMP_DIR}/client-ca.crt" \
  -days 365 \
  -subj "/CN=Edgion Client CA/O=Edgion/OU=Testing" \
  -addext "basicConstraints=critical,CA:TRUE" \
  -addext "keyUsage=critical,keyCertSign,cRLSign" \
  >/dev/null 2>&1

openssl req -newkey rsa:2048 -nodes \
  -keyout "${TMP_DIR}/intermediate-ca.key" \
  -out "${TMP_DIR}/intermediate-ca.csr" \
  -subj "/CN=Edgion Intermediate CA/O=Edgion/OU=Testing" \
  >/dev/null 2>&1

cat > "${TMP_DIR}/intermediate-ca.ext" <<EOF
basicConstraints=CA:TRUE,pathlen:0
keyUsage=keyCertSign,cRLSign
EOF

openssl x509 -req \
  -in "${TMP_DIR}/intermediate-ca.csr" \
  -CA "${TMP_DIR}/client-ca.crt" \
  -CAkey "${TMP_DIR}/client-ca.key" \
  -CAcreateserial \
  -out "${TMP_DIR}/intermediate-ca.crt" \
  -days 365 \
  -extfile "${TMP_DIR}/intermediate-ca.ext" \
  >/dev/null 2>&1

cat "${TMP_DIR}/client-ca.crt" "${TMP_DIR}/intermediate-ca.crt" > "${TMP_DIR}/ca-chain.crt"

echo "[certs] generate mTLS client certificates"
openssl req -newkey rsa:2048 -nodes \
  -keyout "${TMP_DIR}/valid-client.key" \
  -out "${TMP_DIR}/valid-client.csr" \
  -subj "/CN=valid-client/O=Edgion/OU=Testing" \
  >/dev/null 2>&1

cat > "${TMP_DIR}/valid-client.ext" <<EOF
subjectAltName=DNS:client1.example.com,DNS:valid-client.edgion.io,DNS:*.edgion.io,email:test@edgion.io
extendedKeyUsage=clientAuth
EOF

openssl x509 -req \
  -in "${TMP_DIR}/valid-client.csr" \
  -CA "${TMP_DIR}/client-ca.crt" \
  -CAkey "${TMP_DIR}/client-ca.key" \
  -CAcreateserial \
  -out "${TMP_DIR}/valid-client.crt" \
  -days 365 \
  -extfile "${TMP_DIR}/valid-client.ext" \
  >/dev/null 2>&1

openssl req -x509 -newkey rsa:2048 -nodes \
  -keyout "${TMP_DIR}/untrusted-ca.key" \
  -out "${TMP_DIR}/untrusted-ca.crt" \
  -days 365 \
  -subj "/CN=Untrusted CA/O=Unknown/OU=Unknown" \
  >/dev/null 2>&1

openssl req -newkey rsa:2048 -nodes \
  -keyout "${TMP_DIR}/invalid-client.key" \
  -out "${TMP_DIR}/invalid-client.csr" \
  -subj "/CN=invalid-client/O=Unknown/OU=Unknown" \
  >/dev/null 2>&1

cat > "${TMP_DIR}/invalid-client.ext" <<EOF
extendedKeyUsage=clientAuth
EOF

openssl x509 -req \
  -in "${TMP_DIR}/invalid-client.csr" \
  -CA "${TMP_DIR}/untrusted-ca.crt" \
  -CAkey "${TMP_DIR}/untrusted-ca.key" \
  -CAcreateserial \
  -out "${TMP_DIR}/invalid-client.crt" \
  -days 365 \
  -extfile "${TMP_DIR}/invalid-client.ext" \
  >/dev/null 2>&1

openssl req -newkey rsa:2048 -nodes \
  -keyout "${TMP_DIR}/nonmatching-client.key" \
  -out "${TMP_DIR}/nonmatching-client.csr" \
  -subj "/CN=nonmatching-client/O=Edgion/OU=Testing" \
  >/dev/null 2>&1

cat > "${TMP_DIR}/nonmatching-client.ext" <<EOF
subjectAltName=DNS:other-domain.example.com,email:other@example.com
extendedKeyUsage=clientAuth
EOF

openssl x509 -req \
  -in "${TMP_DIR}/nonmatching-client.csr" \
  -CA "${TMP_DIR}/client-ca.crt" \
  -CAkey "${TMP_DIR}/client-ca.key" \
  -CAcreateserial \
  -out "${TMP_DIR}/nonmatching-client.crt" \
  -days 365 \
  -extfile "${TMP_DIR}/nonmatching-client.ext" \
  >/dev/null 2>&1

openssl req -newkey rsa:2048 -nodes \
  -keyout "${TMP_DIR}/chain-client.key" \
  -out "${TMP_DIR}/chain-client.csr" \
  -subj "/CN=chain-client/O=Edgion/OU=Testing" \
  >/dev/null 2>&1

cat > "${TMP_DIR}/chain-client.ext" <<EOF
subjectAltName=DNS:chain-client.edgion.io,DNS:*.edgion.io
extendedKeyUsage=clientAuth
EOF

openssl x509 -req \
  -in "${TMP_DIR}/chain-client.csr" \
  -CA "${TMP_DIR}/intermediate-ca.crt" \
  -CAkey "${TMP_DIR}/intermediate-ca.key" \
  -CAcreateserial \
  -out "${TMP_DIR}/chain-client.crt" \
  -days 365 \
  -extfile "${TMP_DIR}/chain-client.ext" \
  >/dev/null 2>&1

cat "${TMP_DIR}/chain-client.crt" "${TMP_DIR}/intermediate-ca.crt" > "${TMP_DIR}/chain-client-bundle.crt"

openssl req -x509 -newkey rsa:2048 -nodes \
  -keyout "${TMP_DIR}/mtls-server.key" \
  -out "${TMP_DIR}/mtls-server.crt" \
  -days 365 \
  -subj "/CN=mtls.example.com" \
  -addext "subjectAltName=DNS:mtls.example.com,DNS:mtls-optional.example.com,DNS:mtls-san.example.com,DNS:mtls-chain.example.com,DNS:mtls-terminate.example.com" \
  >/dev/null 2>&1

openssl req -x509 -newkey rsa:2048 -nodes \
  -keyout "${TMP_DIR}/fv-server.key" \
  -out "${TMP_DIR}/fv-server.crt" \
  -days 365 \
  -subj "/CN=fv-mtls.example.com" \
  -addext "subjectAltName=DNS:fv-mtls.example.com" \
  >/dev/null 2>&1

echo "[certs] generate backend CA certificate"
openssl req -x509 -newkey rsa:2048 -nodes \
  -keyout "${TMP_DIR}/backend-ca.key" \
  -out "${TMP_DIR}/backend-ca.crt" \
  -days 365 \
  -subj "/CN=Backend Test CA/O=Edgion Testing" \
  -addext "basicConstraints=critical,CA:TRUE" \
  -addext "keyUsage=critical,keyCertSign,cRLSign" \
  >/dev/null 2>&1

echo "[certs] generate backend server certificate (signed by backend CA)"
openssl req -newkey rsa:2048 -nodes \
  -keyout "${TMP_DIR}/backend-server.key" \
  -out "${TMP_DIR}/backend-server.csr" \
  -subj "/CN=backend.example.com/O=Edgion Testing/OU=Backend" \
  >/dev/null 2>&1

cat > "${TMP_DIR}/backend-server.ext" <<EOF
subjectAltName=DNS:backend.example.com,DNS:edgion-test-server.edgion-test.svc.cluster.local,DNS:edgion-test-server.edgion-test.svc,DNS:test-backend-tls.edgion-test.svc.cluster.local,DNS:test-backend-tls.edgion-test.svc,DNS:test-backend-mtls.edgion-test.svc.cluster.local,DNS:test-backend-mtls.edgion-test.svc,DNS:test-backend-mtls-no-client-cert.edgion-test.svc.cluster.local,DNS:test-backend-mtls-no-client-cert.edgion-test.svc
extendedKeyUsage=serverAuth
EOF

openssl x509 -req \
  -in "${TMP_DIR}/backend-server.csr" \
  -CA "${TMP_DIR}/backend-ca.crt" \
  -CAkey "${TMP_DIR}/backend-ca.key" \
  -CAcreateserial \
  -out "${TMP_DIR}/backend-server.crt" \
  -days 365 \
  -extfile "${TMP_DIR}/backend-server.ext" \
  >/dev/null 2>&1

openssl req -newkey rsa:2048 -nodes \
  -keyout "${TMP_DIR}/backend-server-wildcard.key" \
  -out "${TMP_DIR}/backend-server-wildcard.csr" \
  -subj "/CN=*.sandbox.internal/O=Edgion Testing/OU=Backend" \
  >/dev/null 2>&1

cat > "${TMP_DIR}/backend-server-wildcard.ext" <<EOF
subjectAltName=DNS:*.sandbox.internal
extendedKeyUsage=serverAuth
EOF

openssl x509 -req \
  -in "${TMP_DIR}/backend-server-wildcard.csr" \
  -CA "${TMP_DIR}/backend-ca.crt" \
  -CAkey "${TMP_DIR}/backend-ca.key" \
  -CAcreateserial \
  -out "${TMP_DIR}/backend-server-wildcard.crt" \
  -days 365 \
  -extfile "${TMP_DIR}/backend-server-wildcard.ext" \
  >/dev/null 2>&1

for suffix in a b; do
  hostname="hostname-${suffix}.example.com"
  openssl req -newkey rsa:2048 -nodes \
    -keyout "${TMP_DIR}/backend-server-${suffix}.key" \
    -out "${TMP_DIR}/backend-server-${suffix}.csr" \
    -subj "/CN=${hostname}/O=Edgion Testing/OU=Backend" \
    >/dev/null 2>&1

  cat > "${TMP_DIR}/backend-server-${suffix}.ext" <<EOF
subjectAltName=DNS:${hostname}
extendedKeyUsage=serverAuth
EOF

  openssl x509 -req \
    -in "${TMP_DIR}/backend-server-${suffix}.csr" \
    -CA "${TMP_DIR}/backend-ca.crt" \
    -CAkey "${TMP_DIR}/backend-ca.key" \
    -CAcreateserial \
    -out "${TMP_DIR}/backend-server-${suffix}.crt" \
    -days 365 \
    -extfile "${TMP_DIR}/backend-server-${suffix}.ext" \
    >/dev/null 2>&1
done

write_tls_secret \
  "${SECRETS_DIR}/Secret_edgion-test_edge-tls.yaml" \
  "edge-tls" "edgion-test" \
  "${TMP_DIR}/edge-tls.crt" "${TMP_DIR}/edge-tls.key"

write_tls_secret \
  "${SECRETS_DIR}/Secret_cert-store_refgrant-test-cert.yaml" \
  "refgrant-test-cert" "cert-store" \
  "${TMP_DIR}/edge-tls.crt" "${TMP_DIR}/edge-tls.key"

write_tls_secret \
  "${SECRETS_DIR}/Secret_backend_refgrant-test-cert.yaml" \
  "refgrant-test-cert" "backend" \
  "${TMP_DIR}/edge-tls.crt" "${TMP_DIR}/edge-tls.key"

write_tls_secret \
  "${SECRETS_DIR}/Secret_edgion-test_edge-tls-ecdsa.yaml" \
  "edge-tls-ecdsa" "edgion-test" \
  "${TMP_DIR}/edge-tls-ecdsa.crt" "${TMP_DIR}/edge-tls-ecdsa.key"

write_opaque_ca_secret \
  "${SECRETS_DIR}/Secret_edge_client-ca.yaml" \
  "client-ca" "edgion-test" \
  "${TMP_DIR}/client-ca.crt"

write_opaque_ca_secret \
  "${SECRETS_DIR}/Secret_default_header-cert-ca.yaml" \
  "header-cert-ca" "edgion-default" \
  "${TMP_DIR}/client-ca.crt"

write_opaque_ca_secret \
  "${SECRETS_DIR}/Secret_edge_ca-chain.yaml" \
  "ca-chain" "edgion-test" \
  "${TMP_DIR}/ca-chain.crt"

write_tls_secret \
  "${SECRETS_DIR}/Secret_edge_mtls-server.yaml" \
  "mtls-server" "edgion-test" \
  "${TMP_DIR}/mtls-server.crt" "${TMP_DIR}/mtls-server.key"

write_opaque_ca_secret \
  "${SECRETS_DIR}/Secret_edgion-test_fv-client-ca.yaml" \
  "fv-client-ca" "edgion-test" \
  "${TMP_DIR}/client-ca.crt"

write_tls_secret \
  "${SECRETS_DIR}/Secret_edgion-test_fv-server-cert.yaml" \
  "fv-server-cert" "edgion-test" \
  "${TMP_DIR}/fv-server.crt" "${TMP_DIR}/fv-server.key"

write_opaque_ca_secret \
  "${SECRETS_DIR}/Secret_backend-ca.yaml" \
  "backend-ca" "edgion-test" \
  "${TMP_DIR}/backend-ca.crt"

write_opaque_ca_secret \
  "${SECRETS_DIR}/Secret_default_backend-ca.yaml" \
  "backend-ca" "edgion-default" \
  "${TMP_DIR}/backend-ca.crt"

write_ca_configmap \
  "${SECRETS_DIR}/ConfigMap_backend-ca.yaml" \
  "backend-ca" "edgion-test" \
  "${TMP_DIR}/backend-ca.crt"

write_tls_secret \
  "${SECRETS_DIR}/Secret_backend-server-tls.yaml" \
  "backend-server-tls" "edgion-test" \
  "${TMP_DIR}/backend-server.crt" "${TMP_DIR}/backend-server.key"

write_tls_secret \
  "${SECRETS_DIR}/Secret_backend-server-wildcard-tls.yaml" \
  "backend-server-wildcard-tls" "edgion-test" \
  "${TMP_DIR}/backend-server-wildcard.crt" "${TMP_DIR}/backend-server-wildcard.key"

write_tls_secret \
  "${SECRETS_DIR}/Secret_backend-server-sni-a-tls.yaml" \
  "backend-server-sni-a-tls" "edgion-test" \
  "${TMP_DIR}/backend-server-a.crt" "${TMP_DIR}/backend-server-a.key"

write_tls_secret \
  "${SECRETS_DIR}/Secret_backend-server-sni-b-tls.yaml" \
  "backend-server-sni-b-tls" "edgion-test" \
  "${TMP_DIR}/backend-server-b.crt" "${TMP_DIR}/backend-server-b.key"

write_tls_secret \
  "${SECRETS_DIR}/Secret_backend-client-cert.yaml" \
  "backend-client-cert" "edgion-test" \
  "${TMP_DIR}/valid-client.crt" "${TMP_DIR}/valid-client.key"

write_tls_secret \
  "${SECRETS_DIR}/Secret_default_ext-proc-client.yaml" \
  "ext-proc-client" "edgion-default" \
  "${TMP_DIR}/valid-client.crt" "${TMP_DIR}/valid-client.key"

cp "${TMP_DIR}/edge-tls.crt" "${CERTS_DIR}/edge-tls.crt"
cp "${TMP_DIR}/client-ca.crt" "${CERTS_DIR}/client-ca.crt"
cp "${TMP_DIR}/ca-chain.crt" "${CERTS_DIR}/ca-chain.crt"
cp "${TMP_DIR}/mtls-server.crt" "${CERTS_DIR}/mtls-server.crt"
cp "${TMP_DIR}/fv-server.crt" "${CERTS_DIR}/fv-server.crt"
cp "${TMP_DIR}/fv-server.key" "${CERTS_DIR}/fv-server.key"
cp "${TMP_DIR}/backend-ca.crt" "${CERTS_DIR}/backend-ca.crt"
cp "${TMP_DIR}/backend-server.crt" "${CERTS_DIR}/backend-server.crt"
cp "${TMP_DIR}/backend-server.key" "${CERTS_DIR}/backend-server.key"
cp "${TMP_DIR}/backend-server-wildcard.crt" "${CERTS_DIR}/backend-server-wildcard.crt"
cp "${TMP_DIR}/backend-server-wildcard.key" "${CERTS_DIR}/backend-server-wildcard.key"
cp "${TMP_DIR}/backend-server-a.crt" "${CERTS_DIR}/backend-server-a.crt"
cp "${TMP_DIR}/backend-server-a.key" "${CERTS_DIR}/backend-server-a.key"
cp "${TMP_DIR}/backend-server-b.crt" "${CERTS_DIR}/backend-server-b.crt"
cp "${TMP_DIR}/backend-server-b.key" "${CERTS_DIR}/backend-server-b.key"

MTLS_CERTS_DIR="${CERTS_DIR}/mtls"
mkdir -p "${MTLS_CERTS_DIR}"
cp "${TMP_DIR}/valid-client.crt" "${MTLS_CERTS_DIR}/valid-client.crt"
cp "${TMP_DIR}/valid-client.key" "${MTLS_CERTS_DIR}/valid-client.key"
cp "${TMP_DIR}/invalid-client.crt" "${MTLS_CERTS_DIR}/invalid-client.crt"
cp "${TMP_DIR}/invalid-client.key" "${MTLS_CERTS_DIR}/invalid-client.key"
cp "${TMP_DIR}/nonmatching-client.crt" "${MTLS_CERTS_DIR}/nonmatching-client.crt"
cp "${TMP_DIR}/nonmatching-client.key" "${MTLS_CERTS_DIR}/nonmatching-client.key"
cp "${TMP_DIR}/chain-client.crt" "${MTLS_CERTS_DIR}/chain-client.crt"
cp "${TMP_DIR}/chain-client-bundle.crt" "${MTLS_CERTS_DIR}/chain-client-bundle.crt"
cp "${TMP_DIR}/chain-client.key" "${MTLS_CERTS_DIR}/chain-client.key"
cp "${TMP_DIR}/client-ca.crt" "${MTLS_CERTS_DIR}/client-ca.crt"
cp "${TMP_DIR}/intermediate-ca.crt" "${MTLS_CERTS_DIR}/intermediate-ca.crt"
cp "${TMP_DIR}/mtls-server.crt" "${MTLS_CERTS_DIR}/mtls-server.crt"
# Public serving pin for the shared FrontendValidation negative probes.
cp "${TMP_DIR}/fv-server.crt" "${MTLS_CERTS_DIR}/fv-server.crt"
cp "${TMP_DIR}/mtls-server.key" "${MTLS_CERTS_DIR}/mtls-server.key"

write_configmap_from_files \
  "${SECRETS_DIR}/ConfigMap_edgion-test-client-mtls-certs.yaml" \
  "edgion-test-client-mtls-certs" \
  "edgion-test" \
  "${MTLS_CERTS_DIR}"

# Use the canonical LDAP generator, isolated from checkout-local certificates.
LDAP_CERTS_DIR="${CERTS_DIR}/ldap"
LDAP_CERT_SCRIPT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../../integration/conf/Services/ldap-openldap-tls" && pwd)/make-test-certs.sh"
EDGION_LDAP_CERTS_DIR="${LDAP_CERTS_DIR}" EDGION_GENERATED_SECRET_DIR= bash "${LDAP_CERT_SCRIPT}"
write_opaque_ca_secret "${SECRETS_DIR}/Secret_default_ldap-ca-bundle.yaml" \
  ldap-ca-bundle edgion-default "${LDAP_CERTS_DIR}/ca.crt"
write_opaque_ca_secret "${SECRETS_DIR}/Secret_default_ldap-wrong-ca-bundle.yaml" \
  ldap-wrong-ca-bundle edgion-default "${LDAP_CERTS_DIR}/wrong-ca.crt"
write_tls_secret "${SECRETS_DIR}/Secret_default_ldap-client-identity.yaml" \
  ldap-client-identity edgion-default "${LDAP_CERTS_DIR}/client.crt" "${LDAP_CERTS_DIR}/client.key"
cat >"${SECRETS_DIR}/Secret_edgion-test_openldap-tls.yaml" <<EOF
apiVersion: v1
kind: Secret
metadata:
  name: openldap-tls-certs
  namespace: edgion-test
type: Opaque
data:
  ca.crt: $(b64 "${LDAP_CERTS_DIR}/ca.crt")
  server.crt: $(b64 "${LDAP_CERTS_DIR}/server.crt")
  server.key: $(b64 "${LDAP_CERTS_DIR}/server.key")
EOF

echo "[certs] runtime secrets generated: ${SECRETS_DIR}"
