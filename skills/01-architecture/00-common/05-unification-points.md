---
name: unification-points
description: Canonical registry of cross-cutting mechanisms that are (or must become) globally unified — outbound dialing, TLS, SSRF guards, DNS, Redis, dye headers, key extraction, condition predicates, IP matching, real IP, templates, durations, error replies. Check here BEFORE implementing any of these in a new plugin or module.
---

# 05 Unification points registry

Edgion has repeatedly re-invented cross-cutting mechanisms per plugin (the
five incompatible fail-open/fail-close config shapes found on 2026-07-19 are
the proof). This registry is the durable defense: one entry per mechanism,
stating the canonical implementation, the rule new code must follow, and the
known exemptions with their reasons.

The one-line hard rules live in `AGENTS.md` ("Cross-cutting unification
rules"), which is loaded every session; this file carries the detail behind
each rule. Keep the two in sync: a new entry here that is a "violating this
is a bug" rule also gets a line there.

**Status legend**

- **Unified** — a canon exists; new code MUST use it.
- **Fragmented** — multiple incompatible shapes exist; do NOT add another
  variant; the pointed-to task owns the convergence.
- **Frozen** — multiple shapes exist and are grandfathered; do NOT add another
  variant, and no convergence is planned.
- **Gap** — a known violation of an otherwise-unified mechanism, tracked as a
  task.

---

## Outbound & egress

### 1. Outbound HTTP client construction — Unified

- **Canon**: `edgion-common/src/link_sys_core/http_client.rs` —
  `build_outbound_client(OutboundClientConfig)` / `apply_outbound_policy`.
  Centralizes pool / timeout / redirect / proxy-env / TLS policy; the default
  redirect policy is `OutboundRedirect::Deny` because a forgotten redirect
  policy is an SSRF surface. Gateway code reaches it via the re-export
  `crate::link_sys::http_client`. Convenience layers on top:
  `build_pooled_outbound_client` and the process-global `get_http_client()`
  singleton in `edgion-gateway/src/plugins/http/common/http_client.rs`.
- **Rule**: every outbound `reqwest::Client` MUST be built through the
  factory. A bare `reqwest::Client::builder()` outside `#[cfg(test)]` code is
  a bug.
- **Verified consumers** (2026-07-26): the shared singleton,
  `ManagedOutboundClient`, both DSL `http.call` clients, wasm guest callouts
  and OCI pulls, openid_connect (default branch), request_mirror,
  the elasticsearch link_sys provider, health-check probes,
  all_endpoint_status, the controller ACME DNS-01 providers
  (`edgion-controller/src/services/acme/dns_provider.rs`, factory-default
  `Deny` redirects kept deliberately), and the CLI admin-API client
  (`edgion-cli/src/cli/client.rs` `build_cli_client` +
  `edgion-cli/src/cli/commands/login.rs` `probe_status`).
- **Operator-CLI caveat**: `edgion-cli` is the one consumer that sets
  `disable_proxy: false`. Rule 3's proxy-env disable exists to stop a *server*
  from being steered through an attacker-influenced proxy; the CLI runs as the
  operator on the operator's own machine, where honoring `HTTP(S)_PROXY` is a
  requirement, not a leak. `redirect: Deny` still applies — the admin API never
  redirects, and following one would carry the bearer token across an
  https -> http downgrade on the same host.
- **Exemptions**:
  - openid_connect SAN-pinning branch
    (`edgion-gateway/src/plugins/http/openid_connect/plugin.rs`) builds a raw
    client with `use_preconfigured_tls` because a custom
    `ServerCertVerifier` cannot be attached through the factory's high-level
    TLS helpers; it falls back to the shared client on build failure.
  - Test code.

### 2. Outbound request semantics (`ExternalHttpConnection`) — Unified for its class

- **Canon**: schema in
  `edgion-resources/src/resources/common/external_http.rs` +
  `edgion-resources/src/resources/common/outbound_request.rs`; runtime in
  `edgion-gateway/src/plugins/http/common/outbound/client.rs`
  (`ManagedOutboundClient`) and
  `edgion-gateway/src/plugins/http/common/outbound/call.rs`.
- **What it owns**: request templates, retry/backoff, success predicates,
  timeout templates, header/cookie assembly, failure-status semantics for an
  operator-configured HTTP dependency.
- **Consumers**: forward_auth, the LinkSys webhook provider, and the
  `EdgionBackendTrafficPolicy` upstream-authority outbound path.
- **Rule**: a new plugin that calls an operator-configured HTTP dependency
  with any of the above semantics MUST reuse `ExternalHttpConnection` — and
  MUST NOT invent new failure vocabulary
  ([20-edgion-backend-traffic-policy.md](../../02-features/03-resources/20-edgion-backend-traffic-policy.md)
  states this for the authority path; this registry makes it global). The
  vocabulary is frozen at the existing four shapes — see
  [Frozen: failure-policy vocabulary](#frozen-no-new-shapes) below.

### 3. Outbound TLS resolution — Unified

- **Canon**: `edgion-common/src/link_sys_core/outbound_tls.rs`
  (`resolve_outbound_tls`), applied inside the client factory via
  `edgion-common/src/link_sys_core/tls.rs`.
- **Rule**: every outbound TLS configuration (CA bundle, client cert/mTLS,
  verification toggles) MUST resolve through it — including non-reqwest
  dialers.
- **Verified consumers**: the reqwest factory, ext_proc (tonic channel with a
  custom hyper connector), ldap_auth (ldap3), the redis provider (fred), the
  kafka provider (rskafka), the etcd link provider, openid_connect token
  exchange.

### 4. Egress safety (SSRF) — Unified mechanisms, per-path adoption

Three canonical pieces, each with a distinct job:

- **`classify_ip` / `classify_backend_addr`**
  (`edgion-gateway/src/util/net.rs`) — the single IP-classification entry
  point for upstream dial addresses (loopback / link-local + metadata /
  private), IPv4-mapped-IPv6 handled. All three upstream-dial callers
  (`validate_backend_addr`, `validate_endpoint_backend_addr`,
  `pg_upstream_peer`) inherit fixes made here.
- **`EgressResolver`**
  (`edgion-gateway/src/plugins/http/common/outbound/egress.rs`) — implements
  `reqwest::dns::Resolve`, classifying the exact addresses reqwest is about
  to connect to. This closes the DNS-rebinding / TOCTOU window that a
  config-time-only check leaves open. Gateway-wide defaults installed once at
  startup by `configure_egress_defaults`.
- **`AnchorPinnedResolver`**
  (`edgion-gateway/src/plugins/http/common/outbound/pinned_resolver.rs`) —
  for the upstream-authority path: never resolves the composed authority in
  DNS at all; pins every lookup to the anchor Service's ready endpoints.

**Adoption map** (verified 2026-07-19): `EgressResolver` guards
forward_auth / webhook and wasm guest callouts. Upstream dialing goes through
`classify_backend_addr`. DSL `http.call` is a documented design tradeoff
(F-018): IP-literal-only check (`is_ssrf_blocked_ip_literal`) plus a per-hop
redirect guard, no resolution-time classification — see
[dsl-engine-security-model.md](../02-gateway/dsl-engine-security-model.md)
and the decision rules under [skills/04-review/ssrf/](../../04-review/ssrf/)
(7 files). Service-backendRef targets are defended by RBAC/NetworkPolicy,
not in-process checks.

- **Rule**: a NEW egress path whose destination is influenced by operator
  config strings or request data MUST use `EgressResolver` (preferred) or at
  minimum `classify_ip` at dial time; MUST disable reqwest's proxy-env pickup
  (`HTTP_PROXY`/`HTTPS_PROXY`); and MUST deny redirects unless there is a
  documented per-hop re-check. Before flagging or "fixing" an existing path,
  read the ssrf review skills — several apparent gaps are recorded
  tradeoffs.

### 5. DNS resolution — Unified core, deliberate partial adoption

- **Canon**: `edgion-common/src/dns/` — `ResolverSpec`
  (`System` | `StandardDns` via hickory-resolver | `HttpDns` via the LinkSys
  httpdns config), with `fingerprint()` as the cache-key component. Controller
  background resolution:
  `edgion-controller/src/services/dns_resolver/service.rs`; gateway
  data-plane cache: `edgion-gateway/src/backends/discovery/dns_cache.rs`.
- **Rule**: any resolution driven by operator resolver configuration MUST go
  through `ResolverSpec`; do not embed another resolver library or hand-roll
  lookups.
- **Deliberate non-adopters** (OS/system resolver via their client library —
  not gaps): DSL `http.call` (per the F-018 tradeoff above), openid_connect,
  request_mirror, elasticsearch, health probes, ldap3, the ext_proc tonic
  channel.
- **In-canon special case**: the ACME dns-01 propagation probe
  (`edgion-common/src/dns/txt_probe.rs`, controller cold path) shares the
  hickory backend but is deliberately not `ResolverSpec`-driven — its query
  targets are the zone's authoritative nameservers (protocol-determined,
  discovered via the system recursive resolver), never operator resolver
  config. **Rule-3 note (recorded exemption, not compliance)**: the NS IPs it
  dials on port 53 ARE influenced by operator-supplied `spec.domains` (via
  attacker-controllable DNS answers), and no `classify_ip`-style check runs
  at dial time — deliberately: blocking private NS IPs would break the
  legitimate internal-DNS + internal-ACME issuance scenario, `classify_ip` /
  `EgressResolver` live in `edgion-gateway` (unreachable from
  `edgion-common` per the crate DAG), and the blast radius is bounded to
  outbound DNS queries whose responses only fold into a
  Found/NotFound/Unreachable tri-state (no response bytes reach any request,
  log-verbatim, or exfil channel; rate is capped by the cold-path poll
  cadence). Same family as the DSL `http.call` F-018 tradeoff. See
  `skills/04-review/architecture/acme-propagation-check-interval-removed.md`.
- **Not DNS at all**: K8s Service backends dial EndpointSlice pod IPs /
  ClusterIP directly; the upstream-authority outbound path uses
  `AnchorPinnedResolver`.

### 6. Redis access — Unified

- **Canon**: the LinkSys `RedisLinkClient`
  (`edgion-gateway/src/link_sys/providers/redis/client.rs`, a fred `Pool`
  with reconnection/TLS/cluster/sentinel handling), obtained via
  `get_redis_client` from the LinkSys store. Rate-limit Lua machinery
  (SCRIPT LOAD / EVALSHA-with-EVAL-fallback + the three algorithms) is shared
  in `edgion-gateway/src/plugins/common/redis_rate_limiter/mod.rs`.
- **Consumers**: rate_limit_redis, stream connection_rate_limit, DSL
  `redis.*` builtins.
- **Rule**: never create a direct redis connection; new rate-limit algorithms
  extend the shared core, not a plugin-local copy.

---

## Data-plane request mechanics

### 7. Dye suite (traffic tagging) — Unified, entry-level, success + failure

- **Canon**: schema `edgion-resources/src/resources/edgion_plugins/dye.rs`
  (`DyeSuite`/`DyeRule`/`DyeOutcome`); compiled plan
  `edgion-gateway/src/plugins/runtime/dye_plan.rs`
  (`CompiledDyePlan`/`CompiledDyeRule`); recording, the runtime-owned
  anti-spoof strip, and the request/response header write live in
  `edgion-gateway/src/plugins/runtime/session_adapter.rs`
  (`record_outcome`, `strip_request_dye_once` /
  `strip_request_dye_once_core`). `dye` is an entry-level field (sibling of
  `type`/`config`/`alias`/`conditions`), not nested under `config`.
- **Participants**: the five selection plugins (canary,
  dynamic_internal_upstream, dynamic_external_upstream, direct_endpoint,
  region_route — success + fallback/reject; region_route has no reject
  capability yet, pending a separate `SetJumpOutcome::Rejected` bug fix) and
  the dependency-failure plugins (rate_limit_redis, ext_proc, wasm, dsl,
  openid_connect — fail-open + fail-close; forward_auth/ldap_auth —
  fail-close only by policy (auth class never fails open; the degrade switch
  was removed, see the Frozen failure-policy entry below). The
  plugin-type -> capability table is `EdgionPlugin::dye_capabilities`
  (`edgion-resources/src/resources/edgion_plugins/edgion_plugin.rs`).
- **Rule**: any plugin recording an outcome for traffic tagging MUST use the
  shared entry-level `dye` suite / `record_outcome`; do not add a
  plugin-local tagging field or a second dye vocabulary alongside it.
- **Value provenance**: an omitted `value` renders the plugin-recorded value
  (verbatim on `success`; `<type_prefix>:<outcome>:<cause>` on a failure
  outcome); a present `value` is an operator template on the shared
  `${type:name}` engine (see entry 11) — `secretRef` is rejected at
  validation since dye values are client-visible.

### 8. Key extraction (`KeyGet`) — Unified

- **Canon**: `edgion-resources/src/common/key_accessor.rs`. Used by 10+
  plugin configs (key_auth, rate_limit, rate_limit_redis, ctx_set,
  region_route, direct_endpoint, dynamic_*_upstream, conditions, ...).
- **Contract**: nine local variants (includes `RequestPath` after S12); one synchronous `key_get`; no I/O.
  Outbound resolution is the `WebhookKeyGet` plugin writing `KeyGet::Ctx`.
- **Rule**: any new "take a key from header/query/cookie/path/..." field
  MUST reuse `KeyGet` — never a plugin-local source enum. A new outbound
  fetch MUST be a plugin, not a `KeyGet` variant and not a per-plugin
  webhook field. See
  [plugin-metadata/keyget-is-local-io-free.md](../../04-review/plugin-metadata/keyget-is-local-io-free.md).

### 9. IP/CIDR matching — Unified

- **Canon**: `edgion-resources/src/matcher/ip_radix_tree/` (+
  `edgion-resources/src/matcher/ip_cidr_helpers.rs`).
- **Consumers**: http/stream ip_restriction, global_connection_ip_restriction,
  tls_route IP restriction, request_access_policy, real-IP trusted-proxy
  matching, edgion_config_data.
- **Rule**: IP allow/deny matching MUST use the radix tree; no linear
  `Vec<IpNet>` scans, no second matcher.

### 10. Client real-IP determination — Unified

- **Canon**: `edgion-gateway/src/util/real_ip_extractor.rs` (the real_ip
  plugin configures it; `ctx` carries the result).
- **Rule**: never parse `X-Forwarded-For` / `X-Real-IP` ad hoc; consume the
  extractor's result.

### 11. Request-variable templates — Unified engine + safety allow-list

- **Canon**: template schema/parse in
  `edgion-resources/src/resources/common/outbound_request.rs`; rendering in
  `edgion-gateway/src/plugins/http/common/outbound/request.rs` (the
  `${type:name}` engine: `ctx` / `header` / `query` / `cookie` / `secretRef`
  / `remoteIp` / ...).
- **Safety discipline**: security-sensitive fields (anything that steers a
  deadline or a destination) MUST use a restricted
  variable allow-list that rejects client-controlled variable types at
  admission — the worked example is `timeoutMsTemplate` in
  [18-link-sys.md](../../02-features/03-resources/18-link-sys.md).
- **Consumer**: the dye suite's `DyeRule.value`
  (`edgion-resources/src/resources/edgion_plugins/dye.rs`) — classified as a
  reflected-output field: the full variable set minus `secretRef` (dye
  values render straight into a client-visible header, so a secret must
  never reach it; every other variable type is allowed since the field is
  already operator-authored content, not a security decision).
- **Rule**: a new templated field MUST use the shared engine, and MUST
  classify itself: full variable set for request-shaping fields, restricted
  allow-list for security-sensitive ones, full-minus-`secretRef` for
  reflected-output fields.

### 12. Duration config fields — Two sanctioned styles, one parser

- **Canon parser**: `edgion-resources/src/duration.rs` (`parse_duration`).
- **Current state** (deliberate, not a gap): two field styles coexist —
  string durations (`"5s"`, `"100ms"`) on Gateway-API-shaped surfaces
  (route timeouts, health_check, rate_limit intervals, gateway config) and
  integer `*Ms` fields on most plugin configs (ext_proc, wasm,
  `ExternalHttpConnection.timeoutMs`, ...).
- **Rule**: string-duration fields MUST parse via `parse_duration` — never a
  hand-rolled parser. A new field picks ONE of the two existing styles;
  inventing a third format (float seconds, etc.) is a bug.

### 13. Local error replies — Unified contract

- **Canon**: `PluginRunningResult::ErrResponse`
  (`edgion-resources/src/filters.rs`). The runtime renders it at a single
  point, draining the pending response-header queue (dye rules, CORS, ...)
  and recording an `EdgionErrorCode` diagnostic in the access log.
- **Exception**: `ErrTerminateRequest` — only when the response shape is
  inherently dynamic (e.g. forward_auth forwarding the auth service's full
  response, rate_limit_redis's own reject write), and the plugin MUST have
  written the response before returning. The runtime fail-safe turns a
  violation into a 502 + `PluginTerminateWithoutResponse` access-log marker.
  The former `ErrTerminateRequest` opt-out concern (a self-write plugin
  bypassing the queue drain) is resolved: `PluginSession::write_response_header`
  itself drains the pending response-header queue before the plugin's own
  write goes out, so a queued dye rule survives a self-written reply the same
  way it survives `ErrResponse`.
- **Rule**: plugins reject via `ErrResponse` by default. Cross-cutting
  failure machinery (the dye suite's failure-outcome rules) hooks both the
  `ErrResponse` render point and the `write_response_header` drain — no
  termination path can silently drop a queued dye rule.

### 14. Fail-closed error bodies leak no internal detail — Hard rule

- Any fail-close reject body sent to the client MUST NOT include internal
  error detail (upstream addresses, dependency error strings, stack traces).
  Detail goes to `PluginLog` / access-log diagnostics, not the response body.
  Rationale and worked cases:
  [fail-closed-error-body-no-internal-detail.md](../../04-review/security/fail-closed-error-body-no-internal-detail.md).

### 15. Condition predicate layer — Unified

- **Canon**: schema in
  `edgion-resources/src/resources/edgion_plugins/conditions.rs`
  (`Condition`, `MatchCore`, `ConditionSet`); gateway evaluator under
  `edgion-gateway/src/plugins/runtime/conditions/`.
- **Grouping**: every consumer uses the same bounded `ConditionSet`. The top
  level contains exactly one `allOf` or `anyOf`. Top-level `allOf` contains
  direct `Condition` leaves. Top-level `anyOf` contains direct leaves or one
  nested `{ name?, allOf: [...] }` group. No other nesting is valid, and all
  arrays must be non-empty. `PluginConditions.skip` is evaluated before
  `run`, but neither field implies its set's Boolean operator.
- **Naming criterion**: a container field name must state how its
  elements combine (`anyOf` / `allOf`, not `ruleGroups` / `rules`).
- **Observability rule**: match-only atomic descriptors are emitted only for a
  logical decision scope containing more than one atomic `Condition`. A
  zero/one-leaf scope relies on its action/result signal; `cond_skip`, final
  policy decisions, group attribution, and errors are never suppressed. Count
  leaves at compile time, not on the request path.
- **Rule**: new predicates MUST reuse `Condition`. Do not add a
  plugin-local predicate enum, `Include`/`Exclude`, `RestrictionSource`,
  or wildcard `values`. See
  [plugin-metadata/unified-condition-layer.md](../../04-review/plugin-metadata/unified-condition-layer.md).

---

## Already canonical elsewhere (cross-links only)

- **Secret redaction + CRD schema hygiene** — `#[schemars(skip)]` /
  `edgion-resources/src/redact.rs` discipline: see the
  "CRD schema ↔ struct invariants" section of `AGENTS.md` and
  [00-schema-transport-contract.md](../05-resources/00-schema-transport-contract.md).
- **Observability split** — data plane: access log diagnostics +
  restrained metrics; control plane: `tracing` + status conditions. See
  [skills/03-coding/observability/](../../03-coding/observability/01-metrics.md).
- **Security review iron rules** —
  [iron-rules.md](../../04-review/security/iron-rules.md) (review-layer
  companion to this registry).

---

## Frozen (no new shapes)

- **Failure-policy vocabulary** — Frozen — the four existing shapes
  (OnRedisFailure, failure_mode_allow, fail_open, dsl error_policy) are
  grandfathered. New runtime-dependency plugins follow the auth/non-auth
  guideline in 12-edgion-plugin-dev.md; the auth class is fixed fail-close
  (forward_auth's degrade switch was removed 2026-07-20).

## Known gaps & observations (tracked)

> Extraction threshold: a mechanism duplicated 2× is recorded here as an
> observation; 3+ implementations qualify it for extraction as a unification
> point above.

- JWT/JWK parsing helpers exist twice
  (`edgion-gateway/src/plugins/http/openid_connect/jwt_common.rs` vs
  jwt_auth's own) — **decided 2026-07-19: deliberate split, do not extract.**
  The two plugins implement different validation models (JWKS
  discovery/rotation + allowed-alg policy vs static iss-routed groups with
  one-alg-per-group confusion defense); the only textual overlap is two
  `ErrorKind` matches with different output types and log vocabularies.
  Rationale and re-evaluation triggers:
  `skills/04-review/jwt-auth/jwt-oidc-helper-split-deliberate.md`.
