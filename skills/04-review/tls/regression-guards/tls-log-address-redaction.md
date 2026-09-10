---
name: tls-log-address-redaction
description: Review TLS connection log address masking while retaining SNI and useful error diagnostics.
knowledge_type: regression-guards
---

# TLS logs mask backend addresses, not whole diagnostics

## Conclusion and scope

Both TLS termination and passthrough apply the canonical sensitive mode to the
known backend socket address in connect-error text, including its repeated
occurrences in the Pingora error chain. They reuse the rendered diagnostic in
the final `TlsContext.err_log` and retry `TlsContext.log`. `TlsUpstreamInfo.addr`
uses `masked_opt` during serialization; the separate `port` remains readable.
Global TLS logging and both listener JSON outputs share these values.

Do not mask the whole `err_log` or `LogBuffer`: static classifications, error
causes, service references, retry counts, and handshake diagnostics remain
useful. The canonical authority is [log safety](../../../03-coding/01-log-safety.md).

## Rationale and evidence

The two proxy connect-failure branches originally formatted
`connect failed: {upstream_addr_str}: {e}`. The retry branch copied `last_err`
into `ctx.log`, so changing only final `err_log` output missed a duplicate.
The structured `upstream_info.addr` was another copy of the same backend IP.

The checked Pingora revision `518f7274eb9fbd68f3f55e85b8c86a9e4a59e95e`
formats the peer in `connectors/l4.rs`; `BasicPeer::Display` includes its
socket address, and `protocols/l4/ext.rs` also formats the destination in
connect errors. `routes/tls/mod.rs::format_connect_error` renders the error
once and replaces occurrences of that known full socket address in one pass.
It does not scan arbitrary messages for guessed sensitive values or mutate
the runtime peer, route SNI, or PROXY authority.

## Accepted SNI decision and limits

During the explicit `apply-sensitive-mode-to-tls-log-surfaces` / D0116 review,
the user chose plaintext SNI and requested masking only where sensitive
content was demonstrated. TLS proxy `TlsContext.sni` and SSL handshake
`TlsConnMeta.sni` therefore stay plain. This revises the previous blanket SNI
rule; it is an accepted visibility choice, not proof the original code
satisfied that earlier rule. Log readers can see requested hostnames. HTTP
access-log SNI masking is outside this change and remains in place.

The investigated SSL handshake errors have no demonstrated backend-address
source: they describe certificate setup, validity, TLS options, and static
handshake outcomes. Existing mTLS certificate-detail masking remains intact.
The invalid client-IP parsing branch normally formats `unknown`; it does not
prove that ordinary valid client addresses leak through that branch.

Off preserves the original connection error. Replace substitutes only the
address; Wrap retains it inside configured markers and is not secrecy.
`None` remains omitted in context serialization and null in listener fields
that already emitted null. No handshake emission or retry policy changes.

## Verification and re-evaluation

The two `tls_log_masks_only_backend_addresses` tests run each mode in an exact
isolated subprocess. They use real `BasicPeer` and nested Pingora errors for
IPv4/IPv6; check address copies, custom markers, static diagnostics, retry
entries, runtime values, SNI, ports, and absent-field shapes at the context and
listener serialization boundaries. They also serialize `TlsConnMeta` to guard
plain SSL SNI and unchanged handshake diagnostics.

Verified in this fix:

- `cargo test -p edgion-gateway --lib tls_log_masks_only_backend_addresses`:
  2 passed, each running Off/Replace/Wrap in isolated subprocesses.
- The same freshly built Gateway test executable with the `routes::tls::`
  filter: 62 passed, including the two new tests.
- `cargo fmt --all` and `cargo fmt --all -- --check`: passed.
- `cargo check --workspace --all-targets`: passed.
- `cargo clippy --workspace --all-targets`: passed with repository warnings.
- `validate_agent_docs.py`, `check_ssa_force.py`, `check_metrics_inventory.py`,
  and `check_pingora_source_policy.py`: passed.
- Independent implementation, knowledge, and closure-scope review: LGTM.

Existing integration files remain frozen; no new wire-level test is claimed.
Re-evaluate when connector error formatting, address sources, output schemas,
SNI product policy, or sensitive-mode lifecycle changes. New sensitive
fragments require source evidence and local handling, not blanket redaction.

## Resolution references

- `D0116` / `apply-sensitive-mode-to-tls-log-surfaces`: backend-address correction;
  SNI masking explicitly declined, with canonical policy updated.
