# Decide shared HTTP session IP projection

Status: deferred for user decision. Scope: HTTP request identity consumers.

## Evidence

The following request-stage consumers independently parse the session's address
strings into IpAddr:

- `plugins/http/ip_restriction/plugin.rs::get_source_ip` selects RemoteIp or
  DirectPeerIp and parses it for the ACL decision.
- `plugins/runtime/conditions/evaluator.rs::CompiledCondition` evaluates an IP
  predicate by selecting and parsing the same session fields.
- `plugins/http/request_access_policy/plugin.rs::RuntimeProfile::allows` parses
  both identities before evaluating its profile.

Paths above are relative to `edgion-gateway/src/`. A request that uses several
of these consumers can repeat parsing. This is source evidence, not a measured
hotspot or a claim that every request visits every consumer.

`RequestInfo` in `ctx.rs` owns public String fields for physical direct peer,
PROXY-effective downstream peer and resolved RemoteIp. Request metadata fills
these fields. The session adapter's set_remote_ip canonicalizes input and
invalidates remote GeoIP facts only when the canonical value changes. The typed
RealIpResolution removes an intermediate conversion within global resolution;
it is a local result, not shared typed request state for later consumers.

## Decision

Choose whether to retain per-consumer parsing, publish typed identity alongside
text eagerly, or add a canonical lazy projection. Eager fields can add request
size and work even when no IP consumer runs. Lazy state needs an uncomputed state
separate from a computed invalid/missing result, mutation discipline and a measured
memory/allocation tradeoff. Merely adding typed getters with parsing defaults
centralizes syntax but does not remove repeated production parsing.

A shared cache cannot be correct while an untracked public-field write can leave
it stale. Before implementation, inventory every writer, constructor, reset,
clone, direct reply and test fixture; choose an enforceable ownership boundary.
Do not add separate caches inside individual plugins. Other transports may be
read for context, but this task does not authorize changing their state models.

## Required acceptance

- Preserve physical DirectPeerIp, PROXY-effective downstream peer and RemoteIp
  as distinct identities. Forwarded headers remain inert without an authorized
  global or plugin RealIp policy; do not cache trust verdicts with parsed IPs.
- Preserve each consumer's grammar and normalization. ACL, conditions and RAP
  use bare IpAddr parsing; GeoIP's context resolver accepts SocketAddr before
  IpAddr and normalizes mapped IPv6. A shared helper must not make ACL accept
  socket forms, collapse addresses earlier than today or inherit GeoIP failure
  policy. Missing/invalid ACL input must not become the extractor's 0.0.0.0
  fallback, which belongs to a different boundary.
- Cover global resolution, subsequent RealIp plugins, repeated same-canonical
  updates, invalid setter input and held ConfigData snapshots across reload.
  Preserve predicate negation/unresolved outcomes, ACL ordering and existing
  PluginLog outcomes. Keep remote GeoIP invalidation and direct-slot independence.
- Preserve canonical access-log ownership and sensitive serialization. Logging
  must not initialize IP/GeoIP caches or cause a lookup merely to emit a field.
  Add no tracing, raw-address diagnostics or high-cardinality metrics.
- Measure repeated parse counts and elapsed work alongside request layout,
  allocation calls and retained memory. Include no-IP-consumer traffic, one
  consumer, repeated conditions/profiles, IPv4/IPv6 and multiple RealIp updates.
  Report eager and lazy costs separately; do not infer throughput from scan count.
- Keep the existing context-layout baseline investigation separate; do not update
  failing size constants to make an unrelated gate green. Run focused producer,
  setter, consumer and GeoIP tests plus ordinary project checks after an approved
  design and independent IMPL review.

Original constraints: `skills/01-architecture/00-common/05-unification-points.md`,
`skills/04-review/ip-restriction/review-evidence/ip-source-client-ip-false-positive.md`,
`skills/04-review/ip-restriction/mandatory-design/group-name-in-deny-log.md` and
`skills/04-review/observability/regression-guards/geoip-no-global-enabled-switch-access-log-lazy.md`.
Related verification: [context layout baseline](verification-04-plugin-state-layout.md).
No runtime change or benchmark is delivered by this task.
