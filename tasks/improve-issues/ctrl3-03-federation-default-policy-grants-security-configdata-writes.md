# ctrl3-03: Built-in federation Center policy grants write on every EdgionConfigData type, including PROXY-protocol trust and WAF policy

- **Category**: design-error
- **Severity**: P3 (downgraded from P2 by independent verification)
- **Scope**: local
- **Location**:
  - `edgion-controller/src/fed_sync/default_policy.rs:129-162` (`default_center_policy`: create/update/delete on `EdgionConfigData`)
  - `edgion-controller/src/fed_sync/default_policy.rs:1-22` (module doc: "region-route and ip-restriction handlers drive this")
  - `edgion-resources/src/resources/edgion_config_data/mod.rs:223-253` (`ConfigEntry`: `KeyList`, `IpList`, `RequestAccessUrlAllowList`, `ProxyProtocolTrust`, `Selector`, `ClusterRegionSetting`, `WafRuleBundle`, `WafPolicy`, ...)
  - `edgion-controller/src/authz/mod.rs` / `authz/access.rs:11-96` (policy model = verb × kind only)
  - `docs/en/ops-guide/federation-rbac.md:27-47` ("safe built-in default"; "covers **all** plugin metadata")

## Summary

When `[center]` is configured without an explicit `rbac` block, every member Controller grants
the remote federation Center create/update/delete on all `EdgionConfigData`. The stated purpose is
region-route failover and IP-list management, but `EdgionConfigData` is now also the carrier for
the inbound PROXY-protocol trust policy, WAF policies/rule bundles/profile selectors, URL
allow-lists and key lists. The default therefore lets a Center (or anyone holding its mTLS
identity) widen PROXY trust, swap or disable WAF rule sets, or rewrite allow-lists in every member
cluster — far beyond the advertised "safe" scope.

## Current behavior (evidence)

- `default_center_policy` builds `FedAllowRule { verbs: [create, update, delete], kinds:
  [EdgionConfigData] }` (`default_policy.rs:140-143`), used whenever `center.rbac` is `None`
  (`center_policy_from`, `:168-173`).
- The authorization model has no sub-kind dimension: `CenterPolicy::permits(verb, kind)` and the
  access snapshot enumerate `(verb, kind)` only (`authz/access.rs:139-181`). There is no way to say
  "EdgionConfigData of type ClusterRegionSetting/IpList".
- `ConfigEntry` variants (`edgion_config_data/mod.rs:223-253`) include `ProxyProtocolTrust`
  (trusted CIDRs for inbound PROXY headers — the input that sets `RemoteIp` at L4),
  `WafPolicy`/`WafRuleBundle`/`Selector` (which rules run and which profile is active),
  `RequestAccessUrlAllowList` and `KeyList`. The Controller even treats unknown fields in
  `ProxyProtocolTrust`/`WafPolicy` as "security-sensitive"
  (`handlers/edgion_config_data.rs:167-184`), acknowledging their sensitivity.
- The operator doc calls the default "safe" and describes the write grant as covering "plugin
  metadata" (`federation-rbac.md:29-47`), which understates that it includes trust-boundary and
  WAF configuration.

## Industry comparison

- Istio multi-cluster: remote control planes get read-only access to member clusters' service
  registry; configuration writes require explicit per-resource RBAC.
- Kong Konnect / Gloo Fed: the global plane pushes only the object types it owns, and
  per-type/role scoping is the default (e.g. Konnect "Gateway Manager" roles per entity type).
- Kubernetes RBAC itself can scope by resource *and* name; least-privilege defaults grant only
  what a workflow needs.

## Impact

A compromised or misconfigured Center can, by default, weaken network trust (PROXY-protocol
client-IP spoofing that defeats IP ACLs and rate limits keyed on `RemoteIp`) and disable WAF
protection fleet-wide, without the member-cluster operator having opted in. The blast radius grows
silently whenever a new security-relevant `ConfigEntry` type is added.

## Suggested direction

1. Add a type qualifier to the policy model for `EdgionConfigData` (e.g. kind
   `EdgionConfigData/ClusterRegionSetting`, or a `dataTypes` list on a rule) and enforce it in the
   write handlers by inspecting `spec.data.type` of both the current and the requested object.
2. Narrow the built-in default to `ClusterRegionSetting` (and `IpList` if the IP-restriction
   workflow is a committed Center feature); require explicit opt-in for anything else.
3. Fix the operator doc to list the concrete security-relevant types the grant covers.

## Notes

- `federation-rbac.md` documents kind-level granularity as a "known limitation"; the new evidence
  is that the covered types now include `ProxyProtocolTrust` and the WAF policy family, so the
  default is no longer the "minimal allow set" its own module doc claims.
- `skills/04-review/federation/regression-guards/fed-proxy-mtls-fail-close.md` covers mTLS
  fail-close and the Secret read exclusion; it does not discuss the write grant scope.
- `ctrl-svc-01` covers the local Admin API Secret proxy, not the federation default policy.

## Verification (independent)

- **Verdict**: DOWNGRADED (P2 -> P3)
- Code claims hold: `default_policy.rs:139-143` grants create/update/delete on `EdgionConfigData` whenever `rbac` is `None` (`center_policy_from`, `:168-173`), and `ConfigEntry` (`edgion_config_data/mod.rs:223-256`) includes `ProxyProtocolTrust`, `WafRuleBundle`, `WafPolicy`, `Selector`, `KeyList`, `IpList`.
- Downgrade reason: `docs/en/ops-guide/federation-rbac.md:44-46` already discloses that the write grant covers **all** EdgionConfigData as a known kind-level limitation; the Center is an mTLS-authenticated, operator-installed federation authority that can already fail over region routes; and operators can replace the default with an explicit `rbac` block. The new evidence (trust-boundary and WAF types now live in that kind) justifies narrowing the default and fixing the "plugin metadata" wording, but it is a least-privilege hardening, not an exploitable defect against an untrusted party.
- Not a duplicate of `ctrl-svc-01` (local Admin API Secret proxy).

## Re-verification (feature-10-28)

- **Verdict**: STILL-VALID
- **Severity**: P3
- `fed_sync/default_policy.rs`, `authz/`, `edgion_config_data/mod.rs` and `docs/en/ops-guide/federation-rbac.md` are unchanged; `default_policy.rs:139-143` still grants create/update/delete on all `EdgionConfigData`.
