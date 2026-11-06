# ctrl-proc2-02: EdgionGatewayConfig reports `Accepted=True` while its outboundTls Secrets, global plugins and IP lists are missing

- **Category**: design-error
- **Severity**: P2
- **Scope**: local
- **Location**:
  - `edgion-controller/src/conf_mgr/sync_runtime/resource_processor/handlers/edgion_gateway_config.rs:201-226` (`update_status`: only `Accepted`, driven solely by `validation_errors`)
  - `edgion-controller/src/conf_mgr/sync_runtime/resource_processor/handlers/edgion_gateway_config.rs:99-171` (`parse`: resolution outcomes are discarded)
  - `edgion-controller/src/conf_mgr/sync_runtime/resource_processor/credential/egc_tls.rs:33-52` (`resolve`: `Pending(SecretNotFound)` and `Invalid` are dropped by `if let Ok(..)` / `_ => None`)
  - `edgion-controller/src/conf_mgr/sync_runtime/resource_processor/handlers/gateway_class.rs:186-240` (`ParamsRefState::Resolved` only checks that the EGC exists)
  - Contrast: `handlers/link_sys.rs:96-140` (`apply_resolved_refs_condition`), `handlers/edgion_plugins.rs:486` (`bound_credential_status_errors`), `handlers/edgion_acme.rs` (`ResolvedRefs`)
  - Data-plane consequence: `edgion-common/src/link_sys_core/outbound_tls.rs:395-410` (declared ref without material → `OutboundTlsCaMissing`, fail closed)

## Summary
EdgionGatewayConfig (EGC) is the cluster-wide parameters object. It carries the global outbound TLS
(`outboundTls.validation.caCertificateRefs` and `clientCertificateRef`), the global HTTP plugin list, and the
`realIp` trusted IP list references. The handler resolves these references in `parse`, but it throws away
every unresolved or invalid outcome. `update_status` writes only `Accepted`, and only from
syntactic/preparse errors. A missing CA Secret, a missing client-certificate Secret, an unresolved
`globalPluginsRef` or a missing trusted `IpList` therefore all show `Accepted=True` with no `ResolvedRefs`
condition. GatewayClass reports `ResolvedRefs=True` as long as the EGC object exists. Meanwhile the data
plane fails closed on every egress that inherits the global TLS, or, for global plugins, silently runs
without them (see gw-core-02). The kind with the widest blast radius in the system is the one that does
not tell the operator about broken references.

## Current behavior (evidence)
- `egc_tls::resolve` (`egc_tls.rs:33-52`): `if let BoundFetchOutcome::Ok(Some(material)) = fetch(..)`
  ignores `Pending(SecretNotFound)` and `Invalid(..)`, and the client certificate path maps every non-`Ok`
  outcome to `None`. The function returns only material, with no error list.
- `parse` (`edgion_gateway_config.rs:157-168`) writes `resolved_ca_certificates`/`resolved_client_certificate`
  and returns `Continue`. Global plugin refs are only registered for requeue (`plugin_ref_mgr.add_ref`), and
  their existence is never checked. `realIp` `IpList` lookups record `read_scope`/`ref_denied` only.
- `update_status` (`:173-197`): `if validation_errors.is_empty() { accepted_condition } else { Accepted=False }`.
  There is no second condition type.
- GatewayClass (`gateway_class.rs:189-240`): `ParamsRefState::Resolved` → `ResolvedRefs=True "parametersRef resolved"`
  and `Accepted=True`. Nothing propagates the EGC's own reference health.
- Gateway: `resolve_outbound_tls_with_defaults`/`_with_snapshot` fail with `OutboundTlsCaMissing` when the
  inherited global block declares `caCertificateRefs` but `resolved_ca_certificates` is `None`. The same
  happens for a declared client certificate. Every OIDC/JWKS, ext_proc, LDAP, forward-auth and LinkSys
  client that inherits the global block fails closed at runtime. When one of several CA refs is missing,
  the Gateway silently trusts only the remaining bundles, so verification of some upstreams fails with no
  controller-side explanation.

Other kinds that carry the same class of references already report them. LinkSys sets `ResolvedRefs=False`
with typed `ResolvedRefsError`s. EdgionPlugins appends `bound_credential_status_errors`. EdgionAcme sets
`ResolvedRefs` for its account Secret.

## Industry comparison
- Gateway API GEP-713 / GEP-1364 conditions: every object that resolves references publishes
  `ResolvedRefs` (or a `Programmed`/`Ready` equivalent) with a specific reason. Envoy Gateway's
  `EnvoyProxy` and `ClientTrafficPolicy`/`BackendTLSPolicy` set `Accepted=False` or `ResolvedRefs=False`
  when a referenced Secret or ConfigMap is missing.
- Istio `MeshConfig`/`ProxyConfig` problems surface through `istioctl analyze` and status messages. Kong
  `KongClusterPlugin` reports `Programmed=False` when its config Secret cannot be resolved.

## Impact
- A typo in a CA Secret name, or a Secret deleted by GitOps pruning, disables all inherited egress TLS
  gateway-wide. The configuration object still claims `Accepted=True`, and GatewayClass claims
  `ResolvedRefs=True`.
- Missing global plugins (for example auth or WAF at the class level) combine with the fail-open
  data-plane behavior in gw-core-02 into a silent security downgrade with no control-plane signal.
- It is inconsistent with sibling kinds, so operators cannot rely on one condition contract across Edgion
  CRDs.

## Suggested direction
- Make `egc_tls::resolve` return `(BoundTlsMaterial, Vec<ResolvedRefsError>)`, reusing the `PendingReason`
  → `ResolvedRefsError` mapping that `bound_configmap_status_result` already uses. Collect missing global
  plugin targets and missing or wrong-type `IpList` targets in the same list.
- In `update_status`, add a `ResolvedRefs` condition built with the shared `resolved_refs_condition` helper,
  as LinkSys does. For a declared-but-unresolved outboundTls axis, consider also setting
  `Accepted=False` (or a dedicated `Programmed=False`), because the Gateway will reject every inheriting
  egress.
- Have GatewayClass reflect a `ResolvedRefs=False` EGC (for example `Accepted=True` plus
  `ResolvedRefs=False reason=InvalidParameters`), so `kubectl get gatewayclass` exposes it.

## Notes
- Checked `skills/04-review/resources/regression-guards/edgion-gateway-config-global-aggregation-deterministic.md`
  (covers aggregation determinism, not status),
  `skills/04-review/oidc/regression-guards/outbound-tls-policy-construction-fails-closed.md` (confirms the
  data plane intentionally fails closed, which is what makes the missing status signal costly), and
  `skills/04-review/resources/intentional-design/reference-grant-validation-controller-scoped.md` (not
  related). No intentional-design page says EGC should omit `ResolvedRefs`.
- Related: gw-core-02 (Gateway-side fail-open for missing global plugins). This item is the
  controller-status half and is independent of that fix.

## Related findings

- gw-core-02 (gateway-side fail-open on the same missing global plugin references).

## Verification (independent)

- **Verdict**: CONFIRMED
- **Severity**: P2
- `resource_processor/credential/egc_tls.rs:35-53`: `if let BoundFetchOutcome::Ok(Some(material))` for CAs; client cert maps every non-Ok to `None`; no error list returned.
- `handlers/edgion_gateway_config.rs:198-216`: `update_status` sets `Accepted` true unless `validation_errors` non-empty; no `ResolvedRefs` condition (grep finds ResolvedRefs only in GatewayClass-requeue comments).
- Sibling kinds (LinkSys `apply_resolved_refs_condition`, EdgionAcme `ResolvedRefs`) do report reference health, so this is an inconsistency, not a design choice.

## Re-verification (feature-10-28)

- **Verdict**: STILL-VALID
- **Severity**: P2 (unchanged)
- `handlers/edgion_gateway_config.rs`: only change since `feature-08-30` is a new `downstreamTls.validate()` preparse check (`:56-60`); `update_status` (`:201`) still emits only `Accepted` from `validation_errors`, no `ResolvedRefs`.
- `resource_processor/credential/egc_tls.rs` and `handlers/gateway_class.rs` unchanged: unresolved CA/client-cert outcomes still dropped; GatewayClass still reports `ResolvedRefs=True` when the EGC exists.
