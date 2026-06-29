#!/usr/bin/env bash
# make-test-certs.sh — Generate CA, server cert, and client cert for LDAP TLS integration tests.
# Run once from this directory; outputs are gitignored (*.pem, *.key).
# The resulting PEM files are embedded into the K8s Secret YAML fixtures below.
set -euo pipefail

OUT=certs
mkdir -p "$OUT"

# 1. Self-signed CA
openssl req -x509 -newkey rsa:2048 -keyout "$OUT/ca.key" -out "$OUT/ca.crt" \
  -days 3650 -nodes -subj "/CN=edgion-test-ldap-ca"

# 2. Server cert signed by the CA
openssl req -newkey rsa:2048 -keyout "$OUT/server.key" -out "$OUT/server.csr" \
  -nodes -subj "/CN=edgion-test-openldap-tls"
openssl x509 -req -in "$OUT/server.csr" -CA "$OUT/ca.crt" -CAkey "$OUT/ca.key" \
  -CAcreateserial -out "$OUT/server.crt" -days 3650 \
  -extfile <(printf "subjectAltName=DNS:edgion-test-openldap-tls,IP:127.0.0.1")

# 3. Client cert signed by the CA (for mTLS)
openssl req -newkey rsa:2048 -keyout "$OUT/client.key" -out "$OUT/client.csr" \
  -nodes -subj "/CN=edgion-test-ldap-client"
openssl x509 -req -in "$OUT/client.csr" -CA "$OUT/ca.crt" -CAkey "$OUT/ca.key" \
  -CAcreateserial -out "$OUT/client.crt" -days 3650 \
  -extfile <(printf "basicConstraints=CA:FALSE\nkeyUsage=digitalSignature,keyEncipherment\nsubjectKeyIdentifier=hash\n")

# 4. Wrong CA — self-signed, independent root, NOT in the server trust chain
openssl req -x509 -newkey rsa:2048 -keyout "$OUT/wrong-ca.key" -out "$OUT/wrong-ca.crt" \
  -days 3650 -nodes -subj "/CN=edgion-test-wrong-ca"

echo "Certs generated in $OUT/. Embed ca.crt, client.crt, client.key into the Secret YAML fixtures."
echo "Base64 (no-wrap) for YAML:"
echo "ca.crt:       $(base64 -w0 < "$OUT/ca.crt")"
echo "client.crt:   $(base64 -w0 < "$OUT/client.crt")"
echo "client.key:   $(base64 -w0 < "$OUT/client.key")"
echo "wrong-ca.crt: $(base64 -w0 < "$OUT/wrong-ca.crt")"
