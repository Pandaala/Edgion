---
name: frontend-mtls-empty-store-on-missing-ca-key
description: The matcher publishes StrictEffective only for a complete ordered CA set with a non-empty compiled store; all missing, mismatched, or invalid strict inputs publish StrictReject. Reject re-reports of the silent-empty-store form.
knowledge_type: regression-guards
---

# tls-FB01 — Gateway `frontendValidation` silently installed empty CA store on missing `ca.crt`

**Status:** ✅ fixed · 2026-05-27
**Severity:** Medium (BadLogic / silent availability regression) — handshake stays fail-closed, but the listener silently rejects every client cert with no operator-visible signal

## Conclusion

The matcher cold path validates every ordered `resolved_frontend_ca_refs` outcome with the
shared strict PEM/X.509 parser and publishes `StrictEffective` only with a complete compiled
`X509Store`. Missing keys, empty values, malformed bundles, trailing content, private-key
blocks, source mismatches, and partial sets publish `StrictReject` without a store. The TLS
handshake callback only installs that immutable store or rejects; it never parses PEM or builds
trust state per connection.

Re-reports of the form "the handshake callback can build or install an empty trust store" or
"an earlier missing CA source is tolerated when a later source succeeds" are **not accepted**.
Verify `compile_frontend_mtls`: every declared reference must have the outcome at the same index,
with the exact source identity and a valid non-empty canonical CA bundle. Any mismatch makes the
published outcome `StrictReject` with no store.

## What was wrong (before the fix)

The loop body only entered when both `secret.data` was `Some` **and** the data map contained
`CA_CERT`:

```rust
for secret in ca_secrets {
    if let Some(data) = &secret.data {
        if let Some(ca_bytes) = data.get(CA_CERT) {
            add_ca_bundle_to_store(&mut store_builder, &ca_bytes.0)?;
        }
    }
}
let store = store_builder.build();
ssl.set_verify_cert_store(store)?;
ssl.set_verify(SslVerifyMode::PEER | SslVerifyMode::FAIL_IF_NO_PEER_CERT);
```

The `add_ca_bundle_to_store` helper quoted above was itself fail-closed for empty/garbage
PEM, but that guard was reachable only when the key existed. (That helper no longer serves
this path at all: Gateway `frontendValidation` now compiles its store on matcher rebuild,
and the helper survives only on the EdgionTls publication path — see
[ca-bundle-multi-pem.md](ca-bundle-multi-pem.md).)
If every Secret had `data = None` or a `data` map without the `ca.crt` key (e.g. the operator
used `ca.pem`, or referenced a `kubernetes.io/tls` Secret that only carries `tls.crt`/`tls.key`),
the inner body was skipped on every iteration, `store_builder.build()` produced an empty
`X509Store`, `set_verify` was then layered on top, and the function returned `Ok(true)`.
Result: a complete client-cert barrier with no diagnostic — every TLS client was rejected at
handshake; operators learned about the failure only from client-side errors.

## Why the current fix is correct

The matcher cold path owns validation and compilation. It rejects empty reference lists,
unresolved or partial outcome vectors, source/index mismatches, missing material, malformed or
empty bundles, trailing content, private-key blocks, and aggregate-limit violations. Only the
complete set becomes one immutable `X509Store`. The handshake callback performs no CA parsing or
store construction: `StrictEffective` installs the compiled store, `StrictReject` returns an
error, and only `NotConfigured` returns `Ok(false)`.

Gateway API v1.6.2 permits up to 16 frontend CA references. The resource decoder and schema,
the shared `MAX_CA_REFERENCE_COUNT`, Controller resolution, and Gateway compiler must agree on
that bound. A prior mismatch admitted 9–16 references but rejected their runtime projection;
fallback status could still report success while TLS rejected the connection. Boundary tests
cover 9 and 16 references in strict and fallback modes, plus rejection of 17 at the source.

## Fix suggestions not accepted

- "Remove the runtime guard because Controller validation already rejected the source" —
  rejected. Controller resolution now records missing/empty/invalid CA material, but the
  matcher independently validates the complete transport set before compiling a store.
- "Downgrade to `warn!` + `Ok(false)` to keep mTLS optional" — would mask a real
  misconfiguration as a working no-mTLS listener; the operator declared `frontendValidation`,
  so a missing trust anchor is an error, not a "skip silently" signal.
- "Allow a partial ordered set when at least one CA source is usable" — rejected because every
  declared source is part of the strict trust contract and must match its projected identity.

## Re-evaluation triggers

- The controller starts fencing "frontendValidation references Secrets without `ca.crt`" at
  admission time (would make the runtime guard unreachable for that exact misconfiguration,
  but the guard stays correct defense-in-depth — keep it).
- A new frontend CA source or material format is supported — extend the cold compiler while
  preserving complete ordered identity validation and immutable publication.
- The upstream frontend CA reference bound changes — update the decoder, generated schema,
  Controller and Gateway shared limit, and boundary tests together.

## Reference cases

- tls-F010 (`frontend-validation-is-mtls-empty-ca.md`) — only genuinely absent frontend
  validation returns `Ok(false)`; strict empty references are rejected.
- tls-F001 (`terminate-mtls-ca-ordering.md`) — same class: key the verify-mode
  install off the real CA availability, not off the function reaching its end.
- `compile_frontend_mtls` in `runtime/matching/tls.rs` — complete-set validation and publication.

## Resolution references

- `tls-FB01` — 2026-05-27.

## Native Gateway field acquisition

`GatewayTlsRefs` selects frontend references from the declaring Gateway default
or authoritative per-port entry. Listener projections cannot supply authority.
The canonical bound credential pipeline rejects invalid source/target identities
before Grant/store calls or dependency writes, including the native-invalid
`core` alias. Valid missing or denied sources retain typed Secret/ConfigMap CA
recovery edges without also registering ordinary Secret dependencies.

`GatewayHandler::validate` and parse share the same bounded frontend validator:
oversized per-port lists are rejected before traversal, and oversized CA vectors
before reference iteration or projection cloning. This fence is specific to
frontend fields; independent valid serving references retain their own behavior.
Fallback authorization remains payload-read-free. Parse clears injected/stale
material; retarget/delete replace recovery edges, and Grant revocation removes
previously acquired material on the next reconcile. These unit-level acquisition
and matcher checks do not establish live handshake or Gateway API conformance.

Native Gateway TLS material selection preserves FS/etcd raw omitted namespaces:
a canonical default target checks both qualified and bare candidates after
validation and authorization, rejects ambiguity or mismatched actual metadata,
and retains the selected object's metadata. Nondefault targets never read bare
keys. Other bound credential consumers retain their existing exact reads.
Secret/ConfigMap source events preserve raw dependency notifications and add
Gateway-only semantic ordinary notifications for bare objects, plus correctly
typed frontend CA notifications. The two candidate reads are not an atomic
snapshot; dependencies are registered first and subsequent events reconcile
creation, deletion, revocation and ambiguity changes. Matcher identity comparison
interprets only an omitted actual Secret namespace as default, without rewriting
material metadata or accepting an explicit empty namespace.
