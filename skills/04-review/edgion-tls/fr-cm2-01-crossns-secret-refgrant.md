---
name: fr-cm2-01-crossns-secret-refgrant
description: EdgionTlsHandler::parse() now enforces a cross-namespace ReferenceGrant check before resolving spec.secret_ref (server cert) and spec.client_auth.ca_secret_ref (mTLS CA), mirroring GatewayHandler. Reject re-reports claiming EdgionTls embeds an arbitrary other-namespace TLS/CA Secret (incl. private key) without a ReferenceGrant, or that EdgionTls is the lone cross-ns Secret resolver missing the guard.
---

# fr-cm2-01 — EdgionTls cross-namespace Secret refs are ReferenceGrant-gated

**Status:** ✅ fixed · 2026-05-29
**Severity:** P1 (Security) — cross-tenant TLS private-key exfiltration via missing authorization

## Conclusion

`EdgionTlsHandler::parse()`
(`src/core/controller/conf_mgr/sync_runtime/resource_processor/handlers/edgion_tls.rs`)
now calls the module-local `cross_ns_secret_ref_denied(ctx, owner_ns, secret_ref)`
helper immediately before each `secret_store.get()` — once for the server cert
(`spec.secret_ref`) and once for the mTLS CA (`spec.client_auth.ca_secret_ref`).
When the ref targets a different namespace, ReferenceGrant validation is enabled
(`is_reference_grant_validation_enabled()`), and no `ReferenceGrant` authorizes the
`edgion.io/EdgionTls -> ""/Secret` reference, the Secret is **not** read or embedded:
`tls.spec.secret` / `client_auth.ca_secret` stays `None`. `update_status()` recomputes
the same check and reports `ResolvedRefsError::RefNotPermitted` (instead of a perpetual
`SecretNotFound`) on the `ResolvedRefs` condition.

This is the exact guard `GatewayHandler` already applies to `certificateRefs`
(`gateway.rs:1252`) and that `fetch_secret` (`credential/pipeline.rs:126`) applies for
the EdgionAcme / LinkSys / plugin resolvers. EdgionTls was the **only** cross-namespace
Secret resolver missing it.

Re-reports of the form "EdgionTls embeds an arbitrary other-namespace
`kubernetes.io/tls` or CA Secret (including `tls.key`) without a ReferenceGrant",
"a low-privilege EdgionTls writer can point `secretRef.namespace` at a victim namespace
and exfiltrate the private key via conf_sync", or "EdgionTls is the lone handler skipping
`check_reference_allowed`" are **not accepted** — none is still true. Verify the
`cross_ns_secret_ref_denied` call precedes both `secret_store.get()` sites before
re-reporting.

## What was wrong (before the fix)

`parse()` computed `secret_ns = secret_ref.namespace.or(metadata.namespace)` and called
`secret_store.get(secret_ns, name)` directly, with no ReferenceGrant branch on either the
server-cert (lines 128–132) or mTLS-CA (lines 160–163) path. The controller ServiceAccount
reads Secrets cluster-wide, so the Gateway-API ReferenceGrant is the only tenant-isolation
boundary here — and EdgionTls bypassed it. The resolved Secret (incl. `data["tls.key"]`)
was moved into `tls.spec.secret`, which is `skip_serializing_if = Option::is_none` (not
skip-always), so it serialized across conf_sync into the gateway's `ClientCache<EdgionTls>`.
Compounds with `fr-grt-01` (gateway admin API leak) but the controller-side authorization
bypass stood on its own.

`SecretObjectReference`'s own doc comment (`common/secret_object_ref.rs:26`) already states
"Cross-namespace refs require a Gateway API `ReferenceGrant`" — the bug was a direct
violation of that documented contract.

## Why the fix is shaped this way

- **Inline guard, not a pipeline rewrite.** The fix mirrors the established `GatewayHandler`
  pattern rather than rerouting EdgionTls through `fetch_secret` (which would swap the status
  mapping from `ResolvedRefsError` to `FetchOutcome`/`PendingReason` — a larger refactor). The
  cleaner long-term consolidation onto `fetch_secret` remains a valid future task but was out
  of scope for the security fix.
- **Gated by `is_reference_grant_validation_enabled()`.** When validation is disabled, the
  helper returns `false` and behavior is unchanged — consistent with every sibling handler
  (none enforces the grant when the flag is off).
- **`validate()` left unchanged.** It only reads the Secret to emit a cert/key-mismatch
  warning string; it does not embed or expose Secret content, so it is not on the exfiltration
  path. Adding a guard there would be scope creep with no security benefit.
- **Requeue on ReferenceGrant change.** `parse()` also registers the cross-namespace target
  namespace into `ctx.runtime().cross_ns_ref_mgr` (and `on_delete()` / re-`parse()` clears it),
  exactly like `GatewayHandler` (`gateway.rs:256-262, 318-319`). `secret_ref_manager` only
  requeues on **Secret** change events; the ReferenceGrant dispatcher (`CrossNsRevalidationListener`)
  requeues from `cross_ns_ref_mgr`. Without this, a denied EdgionTls would stay stuck in
  `RefNotPermitted` even after the operator adds the authorizing grant. Registration happens for
  any cross-namespace ref regardless of allow/deny so both grant-added and grant-removed requeue.

## Tests guarding this

- `edgion_tls.rs` unit tests: cross-namespace `secret_ref` with no grant leaves
  `spec.secret == None` after `parse()` and yields a `RefNotPermitted` `ResolvedRefs`
  condition; same-namespace and grant-present cases still resolve.

## Relationship to siblings

- `fr-grt-01` is the separate gateway-side companion (admin API serializing the
  already-resolved `spec.secret`); it is not closed by this fix.
- The guard is identical in intent to `GatewayHandler::validate_listener_resolved_refs`
  (cert refs) and `fetch_secret` (credential pipeline); any future cross-namespace Secret
  resolver MUST carry the same check before `secret_store.get()`.
