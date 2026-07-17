#!/bin/bash
# =============================================================================
# Shared conf_sync mTLS helpers for the integration scripts.
#
# DESIGN — the checked-in config IS the mTLS config.
#
# config/edgion-{controller,gateway,controller-etcd}.yaml already carry the
# canonical `conf_sync_security` block:
#
#   controller: certs/grpc/primary/server.{crt,key} + certs/grpc/primary/ca.crt
#   gateway:    certs/grpc/primary/client.{crt,key} + certs/grpc/primary/ca.crt
#
# Those paths are resolved against the **work directory**, so a runner only has
# to make the cert files appear at <work_dir>/certs/grpc/primary/ — it never has
# to rewrite the transport block. `conf_sync_distribute_certs` does exactly that.
# The strip/append machinery that used to inject the block on every start is
# gone from the main path; only two scenarios still rewrite it, because
# rewriting it IS their subject:
#
#   - run_conf_sync_mtls_test.sh            (multi-cert / `active` selection)
#   - run_conf_sync_skip_tls_metric_test.sh (skip_tls + fail-close), via
#     start_all_with_conf.sh's EDGION_CONF_SYNC_PLAINTEXT=1 toggle
#
# Scripts that generate their own binary config MUST NOT hardcode a transport:
# call `conf_sync_security_block server|client` and append its output.
# =============================================================================

# Generate (idempotent) the primary+secondary cert sets directly into
# <target_dir>/certs/grpc/{primary,secondary}/ — the layout the checked-in
# configs reference relative to the work directory.
#
# Call it with the WORK DIR of the process you are about to start. Certs are
# generated in place; there is no shared source store and nothing is ever
# written outside <target_dir>. A developer who wants a bare `cargo run` off
# the checked-in configs bootstraps the repo root explicitly (see the
# `conf_sync_security` comment in config/edgion-*.yaml) — no test script does
# that implicitly.
conf_sync_distribute_certs() {
    local target_dir="$1"
    local script_dir gen_script
    script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
    gen_script="${script_dir}/../gen_certs/generate_conf_sync_certs.sh"

    # Idempotent: the generator reuses an existing complete cert store in the
    # target and validates completeness of both named sets after generation.
    EDGION_CONF_SYNC_CERTS_DIR="${target_dir}/certs/grpc" bash "$gen_script"
}

# Copy an already-generated cert tree from one work dir into another.
#
# For tests that run the Controller and the Gateway with SEPARATE work dirs
# (test_gateway_admin_auth.sh, test_fips_policy.sh): mTLS requires both ends to
# present certs from the SAME set (one CA signs the server and client leaves),
# so the Gateway side must copy the Controller's set — two independent
# conf_sync_distribute_certs calls would mint two unrelated CAs and the
# handshake would fail. The source is the sibling per-run dir of the same test,
# so nothing leaves per-run ownership.
conf_sync_copy_certs() {
    local src_dir="$1" dst_dir="$2"
    if [ ! -d "${src_dir}/certs/grpc/primary" ] || [ ! -d "${src_dir}/certs/grpc/secondary" ]; then
        echo "conf_sync_copy_certs: ${src_dir}/certs/grpc lacks primary/secondary sets — run conf_sync_distribute_certs on the source work dir first" >&2
        return 1
    fi
    mkdir -p "${dst_dir}/certs/grpc"
    rm -rf "${dst_dir}/certs/grpc/primary" "${dst_dir}/certs/grpc/secondary"
    cp -R "${src_dir}/certs/grpc/primary" "${dst_dir}/certs/grpc/primary"
    cp -R "${src_dir}/certs/grpc/secondary" "${dst_dir}/certs/grpc/secondary"
}

# Echo the canonical single-cert `conf_sync_security` block for one role.
#
#   conf_sync_security_block server   # controller: server.crt / server.key
#   conf_sync_security_block client   # gateway:    client.crt / client.key
#
# The paths are work-dir relative and match what conf_sync_distribute_certs lays
# down. This is the ONLY place a transport block is spelled out for the
# self-generating scripts — none of them may hardcode one.
conf_sync_security_block() {
    local role="$1"
    local leaf
    case "$role" in
        server) leaf="server" ;;
        client) leaf="client" ;;
        *)
            echo "conf_sync_security_block: role must be 'server' or 'client', got '${role}'" >&2
            return 1
            ;;
    esac
    cat <<EOF
# conf_sync transport: mTLS (the default everywhere). Certs are placed in the work
# dir by conf_sync_distribute_certs; these paths are work-dir relative.
conf_sync_security:
  tls:
    cert: certs/grpc/primary/${leaf}.crt
    key: certs/grpc/primary/${leaf}.key
    ca: certs/grpc/primary/ca.crt
EOF
}

# Strip the top-level `conf_sync_security:` mapping from a COPIED config.
#
# Only the two scenarios that genuinely rewrite the transport use this (see the
# header). Appending a second `conf_sync_security:` on top of the one the base
# config already ships would be a duplicate YAML key, which serde_yaml rejects —
# so every rewriter must strip first, then append.
#
# Termination is anchored on the next real TOP-LEVEL line (a key or a column-0 comment),
# NOT on "the first line that is not indented" — a blank line matches neither `^[[:space:]]`
# nor a top-level key, so a naive rule would end the skip on a blank line *inside* the block
# and then PRINT the block's remaining indented lines at top level, producing invalid YAML.
# Blank lines are therefore buffered: dropped if the block continues, replayed if it ended
# (which keeps the top-level comment blocks that follow `conf_sync_security:` byte-exact).
#
# BOTH YAML styles must be handled. Block style is
#     conf_sync_security:
#       tls:
#         skip_tls: true
# but the flow (inline) style
#     conf_sync_security: {tls: {skip_tls: true}}
# is exactly what the migration docs recommend pasting in, and it is a SINGLE line with no
# indented children. Matching only the block anchor (`^conf_sync_security:[[:space:]]*$`)
# would let a flow-style line survive the strip; the subsequent append would then emit a
# second `conf_sync_security:` key and serde_yaml rejects the duplicate. A trailing comment
# (`conf_sync_security:   # note`) is still block style — the value is empty — so the two
# cases are told apart by what remains after the colon, not by mere line length.
conf_sync_strip_security_block() {
    local yaml="$1" tmp
    tmp="$(mktemp)"
    awk '
        /^conf_sync_security:/ {
            rest = $0
            sub(/^conf_sync_security:[[:space:]]*/, "", rest)
            if (rest == "" || rest ~ /^#/) { skip = 1; pending = ""; next }  # block style: skip children too
            next                                                             # flow style: single line, done
        }
        skip {
            if ($0 ~ /^[[:space:]]*$/) { pending = pending $0 "\n"; next }  # blank: undecided
            if ($0 ~ /^[[:space:]]/)   { pending = ""; next }               # indented: still inside
            skip = 0; printf "%s", pending; pending = ""                    # top-level: block ended
        }
        { print }
        END { printf "%s", pending }
    ' "$yaml" > "$tmp"
    mv "$tmp" "$yaml"
}

# Replace the transport block of the given COPIED configs with the explicit
# plaintext opt-in. The ONLY caller is start_all_with_conf.sh's
# EDGION_CONF_SYNC_PLAINTEXT=1 toggle, which exists to serve
# run_conf_sync_skip_tls_metric_test.sh — skip_tls is that test's subject.
# Do not use it anywhere else.
conf_sync_replace_with_skip_tls() {
    local yaml
    for yaml in "$@"; do
        conf_sync_strip_security_block "$yaml"
        cat >> "$yaml" <<'EOF'

# INSECURE: explicit plaintext conf_sync. Subject under test of
# run_conf_sync_skip_tls_metric_test.sh — never a default.
conf_sync_security:
  tls:
    skip_tls: true
EOF
    done
}
