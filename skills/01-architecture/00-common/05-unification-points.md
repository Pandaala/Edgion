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
  `build_outbound_client(OutboundClientConfig)` / `apply_outbound_policy`, plus
  `build_outbound_client_with_tls_selection` for actual-owner TLS preparation.
  These entries delegate the same private policy/build core.
  Centralizes pool / timeout / redirect / proxy-env / TLS policy; the default
  redirect policy is `OutboundRedirect::Deny` because a forgotten redirect
  policy is an SSRF surface. Gateway code reaches it via the re-export
  `crate::link_sys::http_client`. The process-global getters in
  `edgion-gateway/src/plugins/http/common/http_client.rs` both use this factory:
  `get_http_client()` has pool 32, a 5-second connect timeout, and a 10-second
  total timeout; the dedicated `get_mirror_http_client()` has pool 32 and no
  internal connect or total timeout because RequestMirror owns the outer
  `connect_timeout + write_timeout` deadline. Both deny redirects and disable
  proxy-environment pickup.
- **HTTP/2 response headers**: the factory always sets an explicit 16 KiB
  decoded response-header-list limit instead of inheriting reqwest's mutable
  default. A Gateway process may raise the process-wide value up to 64 KiB via
  `gateway.outbound_http2_max_header_list_size`; Controller and CLI processes
  retain 16 KiB. The value is restart-only and must be installed before the
  first canonical client is constructed.
- **Rule**: every outbound `reqwest::Client` MUST be built through the
  factory. A bare `reqwest::Client::builder()` outside `#[cfg(test)]` code is
  a bug.
- **Verified consumers** (2026-09-14): Common HttpDns; Controller OAuth2
  credential acquisition; the Gateway default and RequestMirror singletons,
  `ManagedOutboundClient`, all four DSL `http.call` singleton variants, Wasm
  guest callouts, URL module loads and OCI pulls, openid_connect ordinary and
  SAN-pinning paths, GeoIP updates, encrypted and ordinary active health
  checks, all_endpoint_status, and the Elasticsearch LinkSys provider; plus
  the CLI admin client and login status probe. At that audit the custom-builder
  consumers (ordinary health probes and Elasticsearch) called `apply_outbound_policy`;
  the other listed constructions used `build_outbound_client` at that audit.
  HTTPDNS and Webhook now use the owning-selection entry through actual LinkSys
  candidates. Webhook extracts configuration through its fixed-variant owning
  ManagedOutboundClient entry and shares the private managed construction core.
  Elasticsearch now supplies typed headers to the canonical owning build entry,
  retaining actual source and captured TLS without exposing a raw builder.
- **Operator-CLI caveat**: `edgion-cli` deliberately sets
  `disable_proxy: false`. Elasticsearch also currently retains that default and
  Redirect::Limit(10); its TLS-reference migration preserves existing transport
  behavior, not a new general exemption or proxy/egress acceptance. Rule 3's
  proxy-env disable exists to stop a *server*
  from being steered through an attacker-influenced proxy; the CLI runs as the
  operator on the operator's own machine, where honoring `HTTP(S)_PROXY` is a
  requirement, not a leak. `redirect: Deny` still applies — the admin API never
  redirects, and following one would carry the bearer token across an
  https -> http downgrade on the same host.
- **Exemptions**:
  - Test code.

The factory accepts either schema-level `tls` or `preconfigured_tls`, never both.
Its owning-selection entry instead borrows a proved actual field and one captured
global snapshot, rejecting raw `tls`, `tls_defaults` and `preconfigured_tls`
substitutions. An absent/disabled HTTPDNS or Webhook field uses System trust defaults without
consuming inherited trust. Other raw-input consumers remain tracked for migration.
Verified HTTPS clients needing SAN constraints use the shared Gateway
`tls::validation::build_verified_outbound_tls` builder and the latter input.
OIDC, AllEndpointStatus and encrypted active health checks use this path without bypassing outbound transport
policy. TLS/client construction failure must never select an unconfigured client.

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
  (the snapshot-based `resolve_outbound_tls_with_snapshot` and actual-owner resolver family), applied inside the client factory via
  `edgion-common/src/link_sys_core/tls.rs`.
- **Rule**: every outbound TLS configuration (CA bundle, client cert/mTLS,
  verification toggles) MUST resolve through it — including non-reqwest
  dialers.
- **System roots**: `EffectiveCa::System` has one source,
  `edgion_common::system_trust::system_trust()`, which loads the OS bundle at the
  first existing fixed path in `SYSTEM_CA_FILES` once per process (no setting, no
  `SSL_CERT_FILE`). The Gateway
  passes the same file to Pingora `ServerConf.ca_file` for backend TLS. Custom
  trust is `caCertificateRefs`, which replaces System. Do not load roots in a
  `wellKnownCACertificates` consumer (`webpki_roots`, `with_webpki_roots`,
  `with_native_roots`, `rustls-native-certs`); see
  `skills/04-review/tls/regression-guards/system-ca-single-trust-store.md` for
  the exempt non-`System` clients (ACME, kube-client).
- **Local CA file (CLI only)**: `link_sys_core::tls::build_custom_ca_http_rustls_config`
  builds verified HTTP TLS from a local CA file without the resolver. Its sole
  sanctioned consumer is `edgion-cli --ca-cert`, which has no resource or
  EdgionGatewayConfig to resolve; the file replaces System like
  `caCertificateRefs`. Parsing tolerates text between PEM blocks and then applies
  the canonical CA-bundle checks, so the same bytes can pass here and fail as a
  Kubernetes Secret. Gateway and Controller trust must not use it (that would
  reintroduce the rejected `system_ca_file`-style setting); re-evaluate if a
  non-CLI caller appears.
- Backend policy consumers use its pure `resolve_outbound_tls_with_defaults`
  entry with explicit defaults, so LinkSys global trust or client credentials
  cannot override backend identity. Configuration audit logging remains in
  `resolve_outbound_tls_audited_with_defaults` and `resolve_validated_outbound_tls_audited`;
  request-time callers use the pure entry and access logs.
- The reference migration adds an actual-owner `ValidatedLocalTlsSelection` entry
  for canonical resolution. HTTPDNS uses `HttpDnsCandidate` and Webhook uses its
  `WebhookCandidate` plus the fixed-variant managed owning entry. Both use the canonical
  HTTP factory's `build_outbound_client_with_tls_selection`: one actual LinkSys and
  captured global snapshot supply incoming proof, replay comparison and build.
  Raw TLS substitutions are rejected; absent/disabled HTTPDNS/Webhook TLS keeps
  System trust without global inheritance. Common's pure active-local inheritance
  check and masked inherited-axis fingerprint encoder share the same canonical
  precedence; comparison performs no PEM parsing or TLS audit. RedisCandidate's
  Common PreparedRedisTls adapter applies the same actual selection/pair checker,
  captured snapshot and inherited encoder before persistent reuse. Enabled local
  TLS or implicit rediss selects transport; disabled refs stay inert. One resolved
  result emits validated PEM roots and client identity for redis-rs
  across reconnect. The reference task has completed the scoped owning transport
  and replay migrations through R12b; fresh live acceptance remains open.
  Redis and Elasticsearch now share PreparedLinkSysTls's immutable actual-owner,
  reference-topology, captured-default and whole-selection orchestration; their
  closed adapters retain distinct transport/inheritance decisions. ES uses the
  fixed-header owning factory and retains its constructed Client across bulk and
  recovery. Absent/disabled ES TLS, including HTTPS endpoints, consumes no globals.
  Kafka uses that same actual-owner container and inherited encoder before replay.
  Its fixed owning rustls entry reuses the sole Common concrete conversion core,
  including explicit System TLS (rskafka None means plaintext). Absent/disabled
  local TLS consumes no globals. Init and probe recovery retain the constructed
  connector Arc and actual owner; no TLS config/Secret clones or global rereads.
  SASL readiness remains fail-fast. R9b added actual provider auth-field/material
  pairing before runtime replay without changing Controller acquisition authority.
  OIDC, LDAP, ExtProc and Wasm now bind their actual local TLS fields before cache
  reuse or construction, retaining their distinct transport/inactivity rules.
  R12b retired the obsolete detached managed-client and raw rustls constructors
  and the unused global-loading resolver wrapper. Generic HTTP construction,
  preconfigured TLS, process bootstrap and live canonical conversion cores remain.
  Scoped source and unit verification does not establish live revocation or conformance.
- **Verified consumers**: the reqwest factory, ext_proc (tonic channel with a
  custom hyper connector), ldap_auth (tokio-rustls over the `edgion-ldap` bind session), the redis provider (redis-rs), the
  kafka provider (rskafka), the etcd link provider, the OTLP LinkSys Tonic 0.14 transport, openid_connect token
  exchange, and health-local HTTPS/GRPCS probes (explicit defaults).

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

**Adoption map** (verified 2026-09-13): `EgressResolver` guards
forward_auth / webhook, wasm guest callouts, ldap_auth dial-time LDAP
connections, Redis seed/reconnect/Sentinel/Cluster dials, OTLP exporter/probe dials, and openid_connect IdP clients (default factory and SAN-pinning
branches; IP-literal URLs are classified before send because reqwest skips
custom DNS for literals). Upstream dialing goes through `classify_backend_addr`.
DSL `http.call` is a documented design tradeoff (F-018): the default-off
`httpBlockPrivateIps` path keeps an IP-literal-only pre-send check plus a
per-hop redirect guard; when the flag is on, the guarded client also installs
`EgressResolver`. See
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

HTTP DNS endpoint connections use Common's `EgressResolver` with fixed System
bootstrap and literal pre-send classification. Private services remain allowed;
`connection.allowLoopback` defaults false, and link-local/unspecified remain
denied. See [HTTP DNS endpoint policy](../../04-review/ssrf/regression-guards/httpdns-endpoint-egress.md).

- **Canon**: `edgion-common/src/dns/` — `ResolverSpec`
  (`System` | `StandardDns` via hickory-resolver | `HttpDns` via the LinkSys
  prepared HTTPDNS generation), with `fingerprint()` as the cache-key component.
  HTTPDNS clients are prepared when configuration is published or reconciled;
  resolution never constructs a client or audits TLS. Controller
  background resolution:
  `edgion-controller/src/services/dns_resolver/service.rs`. Answer caching:
  `edgion-common/src/dns/cache.rs` (`DnsAnswerCache`: TTL-honoring, per-key
  single-flight, stale-while-revalidate/stale-on-error, short negative
  entries). Instances: the shared egress cache behind `EgressResolver`s with
  operator-configured destinations, the Gateway request-influenced egress cache
  for DSL `http.call` (`request_egress_answer_cache()`), and the Gateway
  upstream cache (`upstream_dns_cache()` in
  `edgion-gateway/src/backends/discovery/dns_cache.rs`) for AI and ExternalJump
  (RegionRoute, DynamicExternalUpstream) resolution and UDP `system`-mode
  ExternalName sessions. ExternalName `gateway` mode uses the background endpoint
  table instead of a cache. Callers do not add another DNS cache type.
- **Rule**: any resolution driven by operator resolver configuration MUST go
  through `ResolverSpec`; do not embed another resolver library or hand-roll
  lookups.
- **Deliberate non-adopters** (OS/system resolver via their client library —
  not gaps): DSL `http.call` (per the F-018 tradeoff above),
  request_mirror, elasticsearch, health probes, the ext_proc tonic channel.
  ldap_auth and openid_connect are no longer in this list: ldap_auth resolves
  through `EgressResolver` (`ResolverSpec::System`) and runs the `edgion-ldap`
  bind session over the admitted stream; openid_connect installs the same resolver on both IdP
  HTTP clients. Redis likewise adapts redis-rs's resolver hook to `EgressResolver`
  with `ResolverSpec::System`, so every actual topology dial is classified.
- **Native Service strategy**: ordinary Services dial proven EndpointSlice IPs.
  References to an actual `Service.spec.type: ExternalName` select, per
  `edgion.io/dns-resolution-mode`, Gateway endpoint-table slices (`gateway`),
  Controller-published slices (`controller`) or a per-attempt OS lookup
  (`system`), on HTTP, gRPC, TCP, UDP and TLS alike; pseudo reference kinds do
  not select a strategy. The separate upstream-authority
  outbound path uses `AnchorPinnedResolver`.

### 6. Redis access — Unified

- **Canon**: the LinkSys `RedisLinkClient`
  (`edgion-gateway/src/link_sys/providers/redis/client.rs`, an Edgion multiplexed pool
  with reconnection/TLS/cluster/sentinel handling), obtained via
  `get_redis_client` from the LinkSys store. Rate-limit Lua machinery
  (shared local script descriptors / EVALSHA-with-EVAL-fallback, request-rate
  algorithms, cumulative quotas, and credential reservations) is shared
  in `edgion-gateway/src/plugins/common/redis_rate_limiter/mod.rs`.
- **Consumers**: rate_limit_redis, stream connection_rate_limit, AI quota and
  credential reservation runtimes, and DSL `redis.*` builtins (client access).
- **Rule**: never create a direct redis connection; new rate-limit algorithms
  extend the shared core, not a plugin-local copy.
- **Dial invariant**: `transport.rs` injects `RedisEgressResolver` into both
  direct multiplexed connections and the Cluster builder before dialing. This
  covers seed/recovery/Sentinel/MOVED targets. ConnectionManager and redis-rs
  Sentinel helpers are not used because they cannot preserve this injection.

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
- **Contract**: twelve local variants; one synchronous `key_get`; no I/O.
  Outbound resolution is the `WebhookKeyGet` plugin writing `KeyGet::Ctx`.
- **Rule**: any new "take a key from header/query/cookie/path/..." field
  MUST reuse `KeyGet` — never a plugin-local source enum. A new outbound
  fetch MUST be a plugin, not a `KeyGet` variant and not a per-plugin
  webhook field. See
  [plugin-metadata/keyget-is-local-io-free.md](../../04-review/plugin-metadata/mandatory-design/keyget-is-local-io-free.md).

### 9. IP/CIDR matching — Unified

- **Canon**: `edgion-algo/src/matcher/radix_ip/` (+
  `edgion-resources/src/matcher/ip_cidr_helpers.rs`).
- **Consumers**: http/stream ip_restriction, global_connection_ip_restriction,
  tls_route IP restriction, request_access_policy, real-IP trusted-proxy
  matching, GeoIpLocation CIDR rules (HTTP and Stream), edgion_config_data.
- **Rule**: IP allow/deny matching MUST use the radix tree; no linear
  `Vec<IpNet>` scans, no second matcher.

### 10. Client real-IP determination — Unified

- **Canon**: `edgion-gateway/src/util/real_ip_extractor.rs` (the real_ip
  plugin configures it; `ctx` carries the result).
- **Rule**: never parse `X-Forwarded-For` / `X-Real-IP` ad hoc; consume the
  extractor's result.

### 11. Request-variable templates — Unified engine + safety allow-list

- **Canon**: template schema/parse in
  `edgion-resources/src/resources/common/request_template.rs`; rendering in
  `edgion-gateway/src/link_sys/template/engine.rs` (the
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
  The [runtime policy matrix](../02-gateway/request-template-runtime.md) defines
  missing values, Secret capability/provenance, escaping, and output budgets.
- **Exemption — LDAP `searchFilter` `{username}`**: not a request-variable
  template. The filter is compiled once into a BER op stream and the
  authenticated-input username is inserted as raw assertion bytes, never
  rendered into text (rendering would reintroduce filter injection). See
  `skills/04-review/ldap-auth/regression-guards/search-mode-identity-and-filter.md`.

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
- **Body representation**: `Option<bytes::Bytes>` — one clone-cheap immutable
  type for every `ErrResponse` producer and the request-body filter's
  `ChunkVerdict::Terminate` → `write_terminate_reply` path. ExtProc immediate
  bodies move from protobuf `Vec<u8>` into `Bytes`, preserving binary HTTP
  payloads without lossy UTF-8 conversion. A fixed literal uses `Bytes::from_static` (no
  allocation); a generation-owned configured body is prepared as `Bytes` at
  plugin construction and clones as a refcount bump; a per-request body uses
  `Bytes::from(String)`, which is a move. The enum deliberately derives no
  `Serialize`/`Deserialize`/`JsonSchema` — it is a process-local return value,
  never a config or wire type. Do not add a second body representation, a
  plugin-local body wrapper, or an ErrResponse-side cache. The explicitly selected
  bounded public-origin ResponseCache has a separate private streaming arena;
  it does not change ErrResponse representation or introduce response BodyStorage.
- **gRPC text boundary**: the request-stage and request-body local-reply
  writers accept only UTF-8 for `grpc-message`; absent or invalid bytes use
  the HTTP-mapped default message. An ExtProc exact gRPC status (including
  zero) keeps absent/invalid text absent instead. Invalid text records the
  existing `PluginGrpcMessageBodyNotUtf8` diagnostic, never the body itself.
  HTTP replies remain byte-exact even when the processor supplies a gRPC
  status. Response-header-stage replacement/abort policy is unchanged.
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
  [fail-closed-error-body-no-internal-detail.md](../../04-review/security/mandatory-design/fail-closed-error-body-no-internal-detail.md).

### 15. Request condition predicate layer — Unified

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
- **Rule**: new request predicates MUST reuse `Condition`. Do not add a
  plugin-local request predicate enum, `Include`/`Exclude`, `RestrictionSource`,
  or wildcard `values`. See
  [plugin-metadata/unified-condition-layer.md](../../04-review/plugin-metadata/superseded/unified-condition-layer.md).
- **Distinct compiler domain**: configuration-only WAF rule-metadata selection
  operates on semantic rule units, not request `KeyGet`/JWT/secret/time inputs.
  Its schema, bounded decoding, validation and evaluation are canonical in the
  external-only `edgion-waf` leaf. Resources, Controller, Gateway configuration
  compilation and CLI reuse it; no importing crate adds another static selector.
  It has no request bindings, request evaluation, or runtime condition descriptors.
  This does not relax the shared `Condition` requirement for any request consumer.
  Keep this scope synchronized with root `AGENTS.md` rule 13.

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
  [iron-rules.md](../../04-review/security/mandatory-design/iron-rules.md) (review-layer
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
  `skills/04-review/jwt-auth/intentional-design/jwt-oidc-helper-split-deliberate.md`.

### Etcd actual-owner and trusted bootstrap TLS

EtcdCandidate and Common Etcd construction retain PreparedLinkSysTls's actual
source and captured defaults; active local and inherited proof precede reuse.
The immutable connection input keeps ordinary/election SDK options for all
reconnect/probe/election paths. Controller's explicit EtcdBootstrapConfig supplies
trusted disk PEM/YAML material without fabricating resource references. Private
borrowed adapters enter the same canonical TLS precedence/materialization core.
The bootstrap input is not resource authority. SDK option/PEM cloning is
unchanged. `System` trust passes the process System bundle explicitly as PEM
(tonic loads no roots implicitly); a custom CA replaces it.

### OTLP actual-owner HTTPS defaults

PreparedOtlpTls retains the shared actual LinkSys loan and captured defaults,
plus OTLP's validated destination and explicit source identity contract. HTTPS
absent/disabled local TLS inherits through empty local policy; HTTP consumes no
TLS globals. OtlpCandidate proves those selections before same-RV/equality reuse
and hashes inherited axes in the secret/material digest. Common canonical TLS
resolution feeds the sole Gateway Tonic 0.14 conversion. The transport retains
its owning input, native Endpoint and metadata across retries/reconnects; SDK
roots, SNI, deadlines, H2 and dial-time egress behavior remain provider-owned.

### Reference-only IP conditions (ARCH-028 B2)

The shared condition evaluator consumes only actual-owner-bound IpList keys and read
policy. IpList `items[].cidrs` is the collection owner; Gateway ConfigData publication
is the matcher owner. Controller condition resolution publishes permission/status
metadata and credential payloads, never an expanded address union. Ordinary required
roots are preflighted as a complete effective runtime before short circuits; RAP
optional roots retain successful empty-union semantics. No threshold or second
matching implementation is used.

### ARCH-028 RealIp reference ownership

HTTP RealIp and global EGC RealIp store only nonempty `trustedIpRefs`; IpGroups live in referenced IpLists. The actual owning EP/EGC catalogue provides the same cold key compiler as Boolean IP conditions. A global EGC requires explicit target namespaces and permits only current Cluster visibility or the controller's validation-disabled scope; its TLS administrative exemption and namespaced grants do not apply. Controller generation processing clears old proof and dependency edges before recomputing from actual fields, including missing-target recovery edges. Shared EGC Accepted remains intrinsic-only; deployment-local trust availability adds no shared condition.

The global policy captures a ConfigData snapshot directly before PluginSession exists; the HTTP policy reuses its dispatch-local adapter view. Both preflight every trust ref once and use that same view for the PROXY-effective downstream peer and all forwarded hops. Any unavailable list makes all trust false for that decision; content/visibility/delete/recovery changes remain live without rebuilding policy or copying CIDRs. Invalid input publishes a sealed plugin generation or disables global extraction, with clean-generation recovery. The narrow realIp CRD preservation and typed invalid marker prevent removed/unknown fields from becoming silently pruned successful configs; resolver-only proof is schema-hidden and recomputed on ingest. Referenced IpLists retain central partial-good entry compilation, replacing the former inline strict-trust admission.

### Geo CIDR reference ownership (ARCH-028)

GeoRule stores geographic values but no CIDRs or matcher. Its CIDR criteria use
IpReferenceSet loans from PreparedIpListReferences and an IpList-domain proof;
Condition/RealIp proofs use the separate Condition domain. One private owning
proof builder verifies source, domain, position, ordered targets and scope pairing.
Gateway CompiledGeoIpRefs retains only cold compiled key grids, then clears unused
CIDR fields from its private normalized rule copy. Resources retains the sole
AND/OR/deny-first/allow-terminal/default algorithm through a membership callback.
All required refs precede MMDB and shortcuts and use one decision snapshot.

### Business IP collections and reference-only ACL decisions

IpList EdgionConfigData is the sole owner of business IP/CIDR collections in IpGroups and their central partial-good matcher index. Plugin conditions, RAP, HTTP/global RealIp, HTTP/Stream Geo CIDR rules, HTTP/connection/TLS IpRestriction and GCIR ProfileRules retain references and scalar policy only. No threshold-based Controller expansion or per-consumer CIDR compiler exists. Startup TCP-peer Controller Admin API allow_admin_ips remains the explicit infrastructure exception, as does inbound PROXY policy's independent authority boundary.

The Controller resolves permission and diagnostics using actual source/target/field-slot loans, clears imported proof every generation, and retains only bounded schema-hidden proof metadata. Same-namespace, explicit Grant, current Cluster-only and validation-disabled read policies are distinct; disabled validation never bypasses identity/type checks. Gateway cold constructors verify actual owner/domain/slot/order before protected cache reuse. A decision borrows one scoped ConfigData snapshot and its authorized matcher iterator, preserving source-first group attribution and each list's longest prefix. No matcher-reference vector, copied CIDRs, per-request permission query or new global cache is needed.

Availability is role-specific: ordinary conditions and Geo require their complete dependency view before shortcuts/MMDB; RAP optional IP data follows its partial-good policy; unavailable RealIp dependencies disable all trust for that decision; optional ACL sources are omitted while authored allowRefs retain whitelist intent. Static typed-invalid generations replace prior runtime policy and fail closed; clean publication recovers. Kubernetes admission-rejected updates never reach this publication boundary and leave the prior admitted generation. ACL denial-group logs retain their explicit security exception; credential-failure attribution remains forbidden.

Disabled standalone ACLs retain passive Controller dependency/status observation without executable proof or Gateway execution. One clear-before-add registry pass preserves all Condition/RealIp/ACL/Geo/Selector edges; no new store, workqueue or runtime IO is introduced.

## Backend DNS address-family policy

`resources::common::DnsLookupFamily` owns exact configuration values and canonical
IP family classification. Common `dns::resolve` requires `DnsQueryPolicy::Native`
for infrastructure or `Family(...)` for backend DNS. Service/Gateway annotations
are parsed at cold publication; queries never parse them. Controller, Gateway DNS
and native AI reuse this core. Standard All requires complete A/AAAA outcomes;
typed HTTPDNS has one primary deadline and same-family fallback. No separate
resolver implementation or Happy Eyeballs mechanism is introduced; answers are
cached only through the shared `DnsAnswerCache` keyed by query policy. AI peer
selection rotates across the cached allowed addresses (no connection racing).
