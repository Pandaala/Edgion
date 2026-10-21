# RealIp local Display scratch

Status: deferred for user decision. No runtime change.
Source baseline: `6f5b84bd3`.

## Evidence

`edgion-gateway/src/plugins/http/real_ip/plugin.rs::run_request` receives a typed
RealIpResolution, then unconditionally formats remote_ip with to_string before
comparing it with session.remote_ip(). Even the unchanged branch creates this
temporary String. The changed branch passes its text to set_remote_ip and the
sensitivity-aware PluginLog formatter.

The production setter in `plugins/runtime/session_adapter.rs` parses and
canonicalizes the supplied address, then replaces the owned session address and
invalidates remote GeoIP only if the canonical value changed. Thus removing this
local temporary does not remove the setter's owned projection or log storage.

This differs from [shared session address projection](ip-01-session-address-projection.md):
local scratch can leave the session API, stored identities and later consumers
unchanged. The extractor already returns IpAddr; global request metadata already
consumes that typed result directly.

## Decision

Evaluate bounded scratch using the standard IpAddr Display implementation,
a streaming text comparison followed by formatting only on change, or retaining
the current String. Scratch needs a justified bound and explicit formatting/error
handling; streaming comparison can format twice on changed outcomes. Compare
unchanged and changed workloads before adding machinery for a short temporary.
No measured allocation count, throughput gain or request-wide allocation-free
claim is made here. Do not introduce a bespoke IP formatter or new dependency
solely to replace this String without separately justifying its cost.

Do not substitute parsed address equality or canonicalize before comparison.
For a mapped IPv6 result and an existing IPv4 session string, the current plugin
calls the setter and logs an update even if the production setter leaves the
canonical session value unchanged. When the session already contains the mapped
Display text, the plugin skips its setter. The existing
mapped_ipv6_keeps_text_comparison_and_setter_boundary test covers this boundary.

## Acceptance before implementation

Use IMPL with independent plan and final reviews and a narrow file allow-list.
Keep exact std Display bytes, textual comparison, setter call count/input,
setter-error GoodNext behavior and fixed error log, and sensitive update logs.
Keep the captured ConfigData view, whole-trust availability, downstream trust
check and header-hop traversal order, recursive/nonrecursive selection, optional
hop cap and invalid-input fallback unchanged. Do not change trust policy.

Differentially check IPv4, IPv6 zero compression and longest forms, mapped IPv6,
unspecified fallback, unchanged and changed values, and setter failure. Exercise
any bounded writer's error path without adding a panic or unsafe UTF-8 conversion.
Verify the production setter preserves remote GeoIP invalidation and direct-slot
independence. No new cache, request field, tracing or eager GeoIP lookup is allowed.

Measure temporary formatting/allocation separately from trust lookup, setter
canonicalization and logging. Report stack/storage costs and any extra formatting
work alongside allocation savings. Preserve existing tests and run the applicable
project gates; do not adjust unrelated context-layout baselines.

## Sources

- `edgion-gateway/src/plugins/http/real_ip/plugin.rs`: RealIpPolicy and run_request.
- `edgion-gateway/src/util/real_ip_extractor.rs`: RealIpResolution and resolve_real_ip.
- `edgion-gateway/src/plugins/runtime/session_adapter.rs`: set_remote_ip.
- `edgion-gateway/src/ctx.rs`: canonicalize_ip and canonicalize_ip_str.
- `edgion-gateway/tests/unit/plugins/http/real_ip/plugin_test.rs`.
- `skills/04-review/real-ip/SKILL.md` and its linked rules.
- `skills/04-review/ip-restriction/review-evidence/ip-source-client-ip-false-positive.md`.
- `skills/04-review/observability/regression-guards/geoip-no-global-enabled-switch-access-log-lazy.md`.
