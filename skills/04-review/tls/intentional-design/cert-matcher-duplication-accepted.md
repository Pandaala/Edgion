---
name: cert-matcher-duplication-accepted
description: Use when reviewing findings that flag `TlsCertMatcher` / `GatewayTlsMatcher` structural duplication and propose extracting a generic `ListenerSniMatcher<T>`; explains why the two matchers stay separate and what shared rule keeps SNI semantics aligned.
knowledge_type: intentional-design
---

# tls-F007 (duplication) — Two SNI matchers stay separate; semantics changes apply to both

**Status:** ⚠️ accepted tradeoff · 2026-05-27
**Severity:** Minor (Redundancy / maintenance only)

Note: distinct from the earlier `tls-F007` (cross-Gateway SNI collision determinism) recorded in
[`cross-gateway-sni-collision-deterministic.md`](../regression-guards/cross-gateway-sni-collision-deterministic.md) — that one fixed a code defect; this one closes a refactor
proposal as a documented design decision.

## Conclusion

`TlsCertMatcher` (`edgion-gateway/src/tls/store/cert_matcher.rs`) and
`GatewayTlsMatcher` (`edgion-gateway/src/runtime/matching/tls.rs`) both wrap
listener/hostname indexes + `ArcSwap*` for lock-free SNI lookup at
TLS handshake time. The two `match_sni` hot paths share the same shape:
`load snapshot → get(listener_key) → RFC6125 lookup → entries.first()`.
The TLS resource store now uses persistent hostname maps; Gateway listener
matching retains `Rfc6125HostnameMatcher`. Both call the shared `lookup` algorithm.

The proposal to factor both out into a generic
`ListenerSniMatcher<T: Clone>` exposing
`ArcSwapOption<HashMap<ListenerKey, Rfc6125HostnameMatcher<Vec<T>>>>` + `match_sni()` is
**not accepted** as a fix-issue task. Re-reports of the structural-duplication
form should be closed by referencing this entry.

## Why the two matchers stay separate

The shape is similar; the substrate is not:

| Aspect | `TlsCertMatcher` | `GatewayTlsMatcher` |
|---|---|---|
| Value type | `Arc<EdgionTls>` | `GatewayTlsEntry` (carries deterministic precedence metadata, compiled serving certificates, and a compiled frontend mTLS outcome/store) |
| Container | `ArcSwap<TlsSnapshotData>` (entries and indexes together) | `ArcSwapOption<TlsMatcherData>` (`None` = empty fast-path, common case) |
| Layers | single hostname matcher | two: `listener_map` (hostname) + `fallback_listener_map` (catch-all listeners without hostname) |
| Rebuild source | EdgionTls CRD store | Gateway listener spec plus Controller-projected ordered serving-certificate and frontend-CA outcomes; unusable serving sets and strict frontend errors remain explicit reject entries |
| Precedence sort | `edgion_tls_precedence` in each affected hostname/attachment slot | `tls_entry_order` (creationTimestamp, namespace, name, listener_name) for cross-Gateway collisions |
| Collision logging | transition-bounded warnings in `TlsStore` | `warn_tls_sni_collision` at rebuild time |

A `ListenerSniMatcher<T>` that captures only the common skeleton leaves the
distinguishing parts — fast-path `Option`, fallback layer, precedence sort,
collision warning — outside the abstraction. The `GatewayTlsMatcher` side would
have to compose two `ListenerSniMatcher` instances (one for hostnames, one for
fallback) plus its own collision/precedence code, so the per-site body shrinks
only marginally while the call graph gains a new generic layer. Body-vs-signature
ratio inverts on the trivial `match_sni` body; the readability cost is real.

This is the same class of decline as `plugins-stream-F003` (rejected
`ValidateAndInit` trait) and `plugins-http-dsl-F009` (declined wholesale
`call_builtin` arm extraction): structural similarity alone does not justify a
shared abstraction when the per-site context differs.

## The actual risk this finding names

The reviewer's concrete worry — "a normalization fix or semantic change applied
to one matcher but missed in the other silently diverges behavior for
EdgionTls-backed vs. Gateway-TLS-backed certificates" — is real but already
managed:

- Both matchers route their hostname lookup through the **single shared**
  `lookup` (also used by `Rfc6125HostnameMatcher::get`) (see `wildcard-depth-gateway-api-vs-rfc6125.md`). RFC
  6125 §6.4.3 single-label wildcard semantics live there; changing them changes
  both call sites at once.
- Both `match_sni` doc comments cite RFC 6125 §6.4.3 + Gateway API §4.3 and
  cross-reference `TlsRouteTable::match_route` to spell out the intentional
  divergence from the multi-label route layer.
- Both resource families can collide across resources on one hostname.
  EdgionTls precedence is maintained by `TlsSnapshotData::apply_indexes` /
  `edgion_tls_precedence`; Gateway listener precedence stays in its own builder.
  The original single-owner description was incorrect, not an accepted absence
  of EdgionTls collision handling.

## Decision Rule

When a review/audit report claims any of the following, close as "accepted tradeoff" referencing this entry:

1. "`TlsCertMatcher::match_sni` and `GatewayTlsMatcher::match_sni` duplicate
   the load→get→RFC 6125 lookup→`first()` chain — extract a generic
   `ListenerSniMatcher<T>`" → **rejected**; the two matchers' wrappers
   (`ArcSwap` vs `ArcSwapOption`-with-fallback) and value-type contracts do not
   share enough to make a generic wrapper a net win.
2. "Future SNI normalization / wildcard fixes will diverge between the two
   matchers" → **rejected as a refactor trigger**; the actual normalization
   lives in `lookup`, also called by `Rfc6125HostnameMatcher::get`. Wildcard depth
   semantics are governed by `wildcard-depth-gateway-api-vs-rfc6125.md`.
3. "`GatewayTlsMatcher`'s fallback layer should also exist on `TlsCertMatcher`
   for symmetry" → **rejected**; EdgionTls is keyed by hostname by
   construction (the CRD requires `spec.hosts`), so there is no catch-all
   case to serve. Adding a no-op fallback layer would only mislead readers.

Re-evaluation triggers (re-open under a new ID, not this one):

- A third SNI matcher appears (e.g., a future per-route TLS provider) — three
  call sites change the cost/benefit on extraction.
- The two matchers grow shared dynamic state (e.g., a unified collision/metrics
  reporter) — at that point pulling the shared core into one helper becomes
  warranted.
- A real production incident traces back to a semantic change applied to one
  matcher but not the other (the comments + this entry are the current
  guardrail; an incident would mean the guardrail failed).

## Reference

- Issue: `tasks/tls/tls-F007-tls-cert-matcher-vs-gateway-tls-matcher-duplication.md`
  (closed as accepted tradeoff 2026-05-27)
- Code:
  - `edgion-gateway/src/tls/store/cert_matcher.rs` — `TlsCertMatcher`
  - `edgion-gateway/src/runtime/matching/tls.rs` — `GatewayTlsMatcher`
  - `edgion-algo/src/matcher/rfc6125_hostname/mod.rs` — shared `Rfc6125HostnameMatcher::get`
- Related rules:
  - `edgion-tls/wildcard-depth-gateway-api-vs-rfc6125.md` (the shared SNI
    semantics this entry depends on)
  - `edgion-tls/cross-gateway-sni-collision-deterministic.md` (the
    earlier `tls-F007` — different finding, same matcher pair)

## Resolution references

- `tls-F007-dup` — 2026-05-27 (⚠️ accepted tradeoff).
- `incremental-publication-03-edgion-tls` — updates the EdgionTls storage shape
  and corrects the old single-owner description; the separate-product matcher
  decision remains applicable.
