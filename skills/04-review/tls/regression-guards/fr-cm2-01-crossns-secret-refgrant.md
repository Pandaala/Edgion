---
name: fr-cm2-01-crossns-secret-refgrant
description: EdgionTlsHandler::parse() now enforces a cross-namespace ReferenceGrant check before resolving spec.secret_ref (server cert) and spec.client_auth.ca_secret_ref (mTLS CA), mirroring GatewayHandler. Reject re-reports claiming EdgionTls embeds an arbitrary other-namespace TLS/CA Secret (incl. private key) without a ReferenceGrant, or that EdgionTls is the lone cross-ns Secret resolver missing the guard.
knowledge_type: regression-guards
---

# fr-cm2-01 — EdgionTls cross-namespace Secret refs are ReferenceGrant-gated

**Status:** ✅ fixed · 2026-05-29
**Severity:** P1 (Security) — cross-tenant TLS private-key exfiltration via missing authorization

## Current contract

The historical 2026-05-29 security fix is now consolidated into canonical bound
acquisition. `EdgionTlsRefs` borrows the actual resource and independently exposes
server and client-CA fields. `fetch_bound_secret` validates the actual source,
strict Secret target and dependency owner before registering pending recovery
edges, authorizing the cross-namespace reference, and reading any payload.
Unsupported group/kind, malformed names or explicit empty namespaces have no
reads or dependency effects. Literal `core` is rejected; omitted group/kind means
empty core group / Secret. Both material slots are scrubbed before acquisition.

Valid denied references retain ordinary Secret and cross-namespace dependencies,
so Secret and ReferenceGrant changes can retry them. Readonly validation and
status create no edges; status uses authorization-only bound checks to distinguish
RefNotPermitted from SecretNotFound without rereading payload. Present material
must match its actual field identity and ordinary Secret provenance.

The existing disabled-ReferenceGrant-validation exception is finite: EdgionTls
slots retain that behavior, as do the separately scoped native Gateway TLS slots.
Do not infer that every other bound credential or TLS consumer honors the flag.
No disabled authority may be frozen into a detached receipt.

Verify the canonical authorization-before-read call chain and event recovery,
rather than looking for the removed detached `cross_ns_secret_ref_denied` helper.
This recorded fix is not proof against new bypasses or stale binding defects.

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

## Historical design and consolidation

The original fix deliberately used an inline guard to minimize a security patch.
That historical scope restriction is superseded by actual-owner bound acquisition.
Its enduring requirement is authorization before both server and CA reads, with
pending Secret/Grant recovery on both allow and deny. Handler processing remains
CPU/local-store only, with one existing workqueue per resource kind.

## Tests guarding this

- `edgion_tls.rs` unit tests: cross-namespace `secret_ref` with no grant leaves
  `spec.secret == None` after `parse()` and yields a `RefNotPermitted` `ResolvedRefs`
  condition; same-namespace and grant-present cases still resolve.

## Relationship to siblings

- `fr-grt-01` is the separate gateway-side companion (admin API serializing the
  already-resolved `spec.secret`); it is not closed by this fix.
- Every cross-namespace Secret consumer must enforce its own canonical field contract
  and authorization policy before payload access; an unrelated consumer cannot
  inherit EdgionTls's finite validation-flag exception.
