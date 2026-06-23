#!/bin/bash
# =============================================================================
# Shared conf_sync mTLS helper for integration startup.
#
# Scope: certificate generation + distribution into a run's work dir, plus a
# single-cert `conf_sync_security` block appender. The dedicated mTLS scenario
# (run_conf_sync_mtls_test.sh) keeps its OWN multi-cert/`active` injection — only
# cert-gen + distribution are shared here.
# =============================================================================

# Generate (idempotent) and distribute primary+secondary cert sets into
# <work_dir>/certs/grpc/{primary,secondary}/.
conf_sync_generate_and_distribute_certs() {
    local work_dir="$1"
    local script_dir gen_script src_dir
    script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
    gen_script="${script_dir}/../gen_certs/generate_conf_sync_certs.sh"
    src_dir="${EDGION_CONF_SYNC_CERTS_DIR:-$(cd "${script_dir}/../../../.." && pwd)/edgion-tests/certs/conf-sync-grpc}"

    bash "$gen_script"
    mkdir -p "${work_dir}/certs/grpc"
    cp -R "${src_dir}/primary" "${work_dir}/certs/grpc/primary"
    cp -R "${src_dir}/secondary" "${work_dir}/certs/grpc/secondary"
}

# Append a single-cert (`primary`) conf_sync_security block to copied configs.
# Controller gets server certs; gateway gets client certs. Paths are work-dir
# relative (resolved by the binaries against --work-dir).
conf_sync_append_single_cert_block() {
    local controller_yaml="$1" gateway_yaml="$2"
    cat >> "$controller_yaml" <<'EOF'

conf_sync_security:
  tls:
    cert: certs/grpc/primary/server.crt
    key: certs/grpc/primary/server.key
    ca: certs/grpc/primary/ca.crt
EOF
    cat >> "$gateway_yaml" <<'EOF'

conf_sync_security:
  tls:
    cert: certs/grpc/primary/client.crt
    key: certs/grpc/primary/client.key
    ca: certs/grpc/primary/ca.crt
EOF
}
