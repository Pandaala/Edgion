# common-03: Egress SSRF guard is opt-in per call site and fragmented; IP literals bypass it and several paths diverge from the canonical policy

- **Category**: design-error
- **Severity**: P2 (worth doing)
- **Scope**: large-direction
- **Location**: `edgion-common/src/link_sys_core/http_client.rs:78-104,140-152` (`OutboundClientConfig` defaults), `edgion-common/src/egress.rs:282` (`impl Resolve for EgressResolver`); literal-guard copies at `edgion-common/src/dns/http_fetcher.rs:104-120,152`, `edgion-controller/src/services/credential_manager/providers/oauth2.rs:185-192`, `edgion-gateway/src/plugins/http/common/outbound/client.rs:647,688`, `edgion-gateway/src/plugins/http/openid_connect/plugin.rs:77-90`, `edgion-gateway/src/plugins/http/wasm/oci/mod.rs:498-516`, `edgion-gateway/src/geoip/updater.rs:175-200`, `edgion-gateway/src/plugins/http/dsl/lang/builtins.rs:2228`; divergent paths `edgion-gateway/src/plugins/http/dsl/runtime.rs:124-151`, `edgion-gateway/src/link_sys/providers/elasticsearch/config_mapping.rs:89-100`

## Summary
The SSRF guard is implemented as a reqwest DNS `Resolve` hook, which reqwest never calls for IP-literal
URLs. So every consumer must (a) remember to set `dns_resolver`, (b) remember `disable_proxy`, (c) pick
a redirect policy, and (d) re-implement a URL-literal check. `OutboundClientConfig::default()` is the
unguarded shape (proxy env honored, system DNS, no classification). The result is 7 hand-written literal
checks with slightly different policies, and at least two production paths that do not meet the
project's own rule D.3 / upstream-address-safety contract.

## Current behavior (evidence)
- `http_client.rs`: `disable_proxy: bool` defaults to `false`; `dns_resolver: None` by default;
  `apply_outbound_policy_core` only wires what the caller passes. Nothing ties "guarded" to a single
  switch.
- `egress.rs:282-299`: classification lives only in `Resolve::resolve`. reqwest dials IP-literal hosts
  directly, which is why each caller re-checks: `http_fetcher.rs:104` (comment: "reqwest skips custom
  DNS for literals"), `oauth2.rs:185` ("The HTTP connector skips DNS for literals"),
  `openid_connect/plugin.rs:77`, `outbound/client.rs:688`, `wasm/oci/mod.rs:508`,
  `geoip/updater.rs:185`, `dsl/lang/builtins.rs:2228`. The copies differ: HTTPDNS ignores
  `block_private`; geoip and OCI honor it; DSL blocks private unconditionally when enabled.
- **DSL `http.call` default has no metadata protection**: `httpBlockPrivateIps` defaults to `false`
  (`edgion-gateway/src/plugins/http/dsl/plugin.rs:75`), and in that mode `build_dsl_http_client`
  installs no resolver (`dns_resolver: block_private_ips.then(..)`, `runtime.rs:150`) and follows 10
  redirects with no per-hop check (`runtime.rs:127`). A script that builds a URL from request data can
  reach `169.254.169.254` or loopback — while every other egress path always blocks
  loopback + link-local and only makes *private* ranges optional. The knob is binary, so an operator
  who must reach internal services (the documented reason it defaults off) has no way to keep the
  metadata block.
- **Elasticsearch LinkSys**: `es_client_config` (`config_mapping.rs:89-100`) uses
  `OutboundRedirect::Limit(10)`, honors proxy env (default), and installs no egress resolver, even
  though the destination is config-influenced (rule D.3: "uses `EgressResolver`, disables proxy-env
  pickup, and denies redirects unless per-hop re-checked"). A 307/308 from a compromised or
  misconfigured ES endpoint can re-POST bulk access-log bodies to any host, including metadata.

## Industry comparison
- Stripe Smokescreen and Go SSRF guards hook `net.Dialer.Control`, i.e. the classification runs on the
  final socket address for *every* connection — literal or resolved, first hop or redirect — so callers
  cannot forget it.
- Envoy/Istio express egress as configuration (`REGISTRY_ONLY` outbound policy, `ServiceEntry`,
  DFP `allow_*` lists) applied by the one data-plane connector, not per feature.

## Impact
Every new outbound feature re-derives four separate security decisions; review history in
`skills/04-review/ssrf/` (FB-01 proxy env, F-012 redirect hop, F-018 literal-only, httpdns endpoint)
shows the same bug class being fixed one call site at a time. Today it leaves DSL default mode and ES
outside the contract, and gives inconsistent semantics across the literal-check copies.

## Suggested direction
1. Add a single `egress: Option<EgressGuard>` field (policy + resolver spec + allow_loopback + observer)
   to `OutboundClientConfig`. When set, `apply_outbound_policy_core` atomically: installs the
   `EgressResolver`, forces `no_proxy()`, and replaces `Limit(n)` with a redirect policy that
   classifies literal hop hosts with the same classifier.
2. Close the literal hole at the connector, not at call sites: wrap reqwest's connector with
   `ClientBuilder::connector_layer` (reqwest 0.12.28 is in the lockfile) to classify literal `Uri` hosts
   before dialing, or expose a `GuardedClient::execute` wrapper that classifies `request.url()` once.
   Delete the seven ad-hoc literal checks.
3. Make DSL `httpBlockPrivateIps` tiered like the rest of the gateway: loopback + link-local/metadata
   always blocked, `httpBlockPrivateIps` only toggles private/ULA.
4. Put the ES client (and any remaining `build_outbound_client` caller) on the guarded mode; keep
   `edgion-cli` as the explicit, documented unguarded exception.

## Notes
Checked `skills/04-review/ssrf/` (all pages). `superseded/dsl-http-call-ip-literal-only.md` records
"`httpBlockPrivateIps` remains default-off because calling internal services is a supported DSL use
case" — new evidence: that rationale only covers *private* ranges; the binary knob also disables the
loopback/metadata block that `mandatory-design/upstream-address-safety.md` treats as non-negotiable for
other paths. `regression-guards/forward-auth-webhook-ssrf.md` (operator-URI internal access) is
respected: this proposal does not block RFC1918 by default. Related: common-01 (classifier coverage).

## Verification (independent)

- **Verdict**: CONFIRMED
- **Severity**: P2
- `edgion-gateway/src/plugins/http/dsl/runtime.rs:123-150`: with `block_private_ips=false` (default, `dsl/plugin.rs:75` `unwrap_or(false)`), redirect = `Limit(10)` and `dns_resolver: None` — no loopback/link-local block at all.
- `edgion-gateway/src/link_sys/providers/elasticsearch/config_mapping.rs:89-100`: `redirect: Limit(10)`, no `dns_resolver`, `disable_proxy` default false — contrary to unification rule 3.
- `edgion-gateway/src/plugins/http/dsl/lang/builtins.rs:2219-2240`: one of several hand-written IP-literal checks with its own policy.
- `skills/04-review/ssrf/superseded/dsl-http-call-ip-literal-only.md` justifies default-off only for private ranges; it does not accept losing the loopback/metadata block, which `mandatory-design/upstream-address-safety.md` requires.

## Re-verification (feature-10-28)

- **Verdict**: STILL-VALID
- **Severity**: P2
- `http_client.rs` changed only TLS defaults (`apply_default_http_tls`) and `dns_resolver2`→`dns_resolver`; `disable_proxy` still defaults to false and `dns_resolver` to `None` (`http_client.rs:86,104,149-154`).
- `dsl/runtime.rs`, `dsl/plugin.rs` and `elasticsearch/config_mapping.rs` are unchanged; the ES diff only bounds the cluster-health body read (`elasticsearch/ops.rs`), not redirect/proxy/resolver policy.
- OIDC literal check still at `openid_connect/plugin.rs:77-90`.
