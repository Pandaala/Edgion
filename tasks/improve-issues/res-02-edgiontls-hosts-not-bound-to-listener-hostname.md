# res-02: EdgionTls `hosts` are not constrained to the attached listener, so any attaching namespace can take over another hostname's certificate

- **Category**: design-error
- **Severity**: P1
- **Scope**: local
- **Location**:
  - `edgion-resources/src/resources/edgion_tls.rs:26` (`validate_operator_attachment_dimensions`: only counts/duplicates), `:172` (`EdgionTlsSpec.hosts` free-form)
  - `edgion-controller/src/conf_mgr/sync_runtime/resource_processor/handlers/edgion_tls.rs:255-292` (attachment = allowedRoutes namespace check + protocol filter only)
  - `edgion-controller/src/conf_mgr/sync_runtime/resource_processor/handlers/route_utils.rs:889-990` (`resolve_listeners_inner`: `listener_allows_route_namespace`, no hostname intersection)
  - `edgion-gateway/src/tls/store/cert_matcher.rs:36-56` (`match_eligible_sni` keys by physical `ListenerKey` + SNI)
  - `edgion-gateway/src/tls/runtime/gateway/tls_pingora.rs:538-566` (Layer 1 EdgionTls wins over Layer 2 Gateway listener certificate when the listener has no frontend mTLS decision)

## Summary

EdgionTls attaches to Gateway listeners like a Route (allowedRoutes namespace check), but unlike a Route its
`hosts` are never intersected with the listener `hostname`, and its index is keyed by the *physical* listener
(address+protocol), which is shared by every listener on the same port. On the data plane an EdgionTls match
takes precedence over the Gateway listener's own `certificateRefs`. Any namespace allowed to attach routes to
one HTTPS listener can therefore install a certificate for any other hostname served on the same port —
including the Gateway owner's or another tenant's hostname — and the gateway will serve it.

## Current behavior (evidence)

- Schema/validation: `EdgionTlsSpec.hosts` only has syntax/length bounds; `validate_operator_attachment_dimensions`
  checks parentRef/host counts and duplicates (`edgion_tls.rs:26-70`). No ownership or listener-hostname rule.
- Controller: `EdgionTlsHandler::parse` calls `resolve_listener_attachments`, which accepts a listener when
  `listener_allows_route_namespace(&l.allowed_routes, route_namespace, ..)` passes and then keeps only
  `HTTPS`/`TLS` protocols. `listener.hostname` is collected for routes but never compared with
  `EdgionTls.spec.hosts`; `allowedRoutes.kinds` is not consulted either (EdgionTls is not a Route kind).
- Gateway: `TlsCertMatcher::match_eligible_sni(&lk, sni, ..)` looks up `listeners[lk].hosts[sni]` where `lk`
  is the physical `ListenerKey`; eligibility only checks that the attachment's listener is eligible, not that
  the SNI belongs to that listener.
- Precedence (`tls_pingora.rs`): if the Gateway entry has a frontend mTLS decision, only an exact-attachment
  overlay is allowed (good). Otherwise step 2 "Layer 1: EdgionTls" returns the EdgionTls certificate before
  step 3 "Layer 2: GatewayTls" is considered.
- Contrast: ACME HTTP-01 serving requires the listener hostname to match
  (`skills/04-review/acme/regression-guards/acme-http01-gatewayref-serving-ownership.md`), and Gateway API
  Routes must intersect listener hostnames. EdgionTls is the only attachment that skips both.

Example: Gateway `infra/shared` has listeners `a` (`hostname: api.example.com`, `certificateRefs: api-cert`,
allowedRoutes Same) and `b` (`hostname: *.tenants.example.com`, allowedRoutes All), both on 443. Tenant `t1`
creates `EdgionTls{parentRefs:[shared/b], hosts:[api.example.com], secretRef: t1/anything}`. Both listeners
share `ListenerKey(0.0.0.0:443, HTTPS)`; handshakes for `api.example.com` now get `t1`'s certificate.

## Industry comparison

- Gateway API: certificates live on the Gateway listener (owned by the cluster/gateway operator); routes can
  only claim hostnames that intersect the listener hostname.
- Contour: a namespace may serve a certificate from another namespace only via explicit
  `TLSCertificateDelegation`; HTTPProxy root/host ownership is restricted to root namespaces.
- Istio: server certificates are bound in the `Gateway` resource (`credentialName`) owned by the gateway
  namespace; VirtualServices cannot supply certificates.

## Impact

- Certificate takeover / denial of service for other tenants' and the platform's hostnames on a shared port
  (clients receive the wrong certificate; with any certificate the attacker legitimately owns for that name —
  e.g. an internal CA — it becomes interception).
- Silent override of the Gateway owner's `certificateRefs`, with only a collision warning in logs.
- Minimum TLS version/ciphers for the hijacked hostname are also taken from the attacker's EdgionTls.

## Suggested direction

1. Require every `EdgionTls.spec.hosts` entry to intersect the hostname of each listener it attaches to
   (reuse the canonical Route hostname intersection in `edgion-algo`); listeners without a hostname accept
   any host only when they are not shared with other hostname-bearing listeners on the same port, or treat
   them as Gateway-owner-only.
2. Index EdgionTls by logical listener attachment (Gateway/listener identity), not only by physical
   `ListenerKey`, so an attachment to listener `b` can never answer SNI for listener `a`.
3. Give the Gateway listener's own `certificateRefs` precedence over EdgionTls for hostnames that listener
   declares, or require an explicit Gateway-owner opt-in (annotation/field on the listener) before tenant
   EdgionTls may serve on it — analogous to Contour's delegation.
4. Report a rejected host on EdgionTls status (`Accepted=False`/`NotAllowedByListeners`).

## Notes

- Checked `skills/04-review/tls/` (intentional-design: `gateway-tls-candidate-ownership`,
  `cert-matcher-duplication-accepted`, `tls-version-operator-choice`; regression guard
  `cross-gateway-sni-collision-deterministic`). They cover memory/precedence determinism among *colliding*
  entries and the frontend-mTLS overlay guard, not hostname ownership of EdgionTls attachments.
- Frontend mTLS bypass is already prevented (overlay only on the exact winning listener); this finding is
  about the non-mTLS certificate path.
- Not tracked under `tasks/`.

## Verification (independent)

- **Verdict**: CONFIRMED — Severity P1 unchanged.
- `handlers/edgion_tls.rs:255-292` resolves attachments through `resolve_listener_attachments` and then filters by HTTPS/TLS protocol only. `spec.hosts` is never compared with the listener `hostname`.
- `edgion-resources/src/listener_key.rs:25-27`: `ListenerKey` is `"{addr}/{PROTOCOL}"` (physical), so listeners sharing a port share the key. `cert_matcher.rs:36-56` `match_eligible_sni` accepts any attachment whose `listener_key == key`, regardless of which logical listener (`listener_name`) it attached to.
- `tls_pingora.rs:556-580`: Layer 1 (EdgionTls) returns before Layer 2 (the Gateway listener certificateRefs) is consulted.
- The `skills/04-review/tls` pages (`gateway-tls-candidate-ownership`, `cross-gateway-sni-collision-deterministic`) cover collision determinism, not hostname ownership.

## Re-verification (feature-10-28)

- **Verdict**: STILL-VALID
- **Severity**: P1
- `handlers/edgion_tls.rs`, `route_utils.rs`, `cert_matcher.rs` and `listener_key.rs` are unchanged; `edgion_tls.rs` only changed doc comments for `minTlsVersion`/`ciphers` inheritance.
- `tls_pingora.rs` was refactored to apply `apply_selected_downstream_policy` after identity selection, but the selection order is the same: non-mTLS path takes `match_eligible_sni(&lk, sni, ..)` (EdgionTls, physical listener key) before the Gateway listener certificate (`tls_pingora.rs:538-566`). Line refs updated.
- New TLS review pages (`downstream-ecdh-policy-ownership`, `ssl-ctx-exdata-handoff-audit-ownership`) cover policy application, not EdgionTls hostname ownership.
