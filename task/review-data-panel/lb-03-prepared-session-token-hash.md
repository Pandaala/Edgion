# Evaluate preparing the stable session-affinity token hash

Status: deferred for user decision. Priority: P3. Type: performance investigation.
Inspected baseline: `83ae44f45`.

## Evidence and potential benefit

The session-persistence branch of `pg_upstream_response_filter` formats the
selected `upstream.lb_backend_addr` inet socket and calls
`session_persistence::generate_scoped_session_token`. The canonical derivation
hashes optional scope bytes plus a NUL delimiter, then address bytes, with SHA-256.
It renders the first eight digest bytes as 16 lowercase hexadecimal characters.
Ordinary Service `sticky_candidate` currently supplies no scope.

This runs only for InjectNew/InjectUpdate on final non-informational responses,
after retry-on-status selection. Established healthy sticky requests do not
need new tokens. `lb::runtime_state::build_sp_affinity_map` also formats and
hashes endpoint addresses, but does so on a lazy affinity-map miss, not every
ordinary selection or current configuration publication. It considers at most
1,024 backends. Scoped views have a separate bounded, uncached build path.

An immutable prepared unscoped digest or hex token on the selected endpoint
could avoid address formatting and SHA-256 during response issuance and reuse
that value in ordinary affinity-map construction. The stable digest is independent
of expiry; `build_wire_token` must still sample issuance time through the current
caller and perform the existing saturating expiry addition.

## Value decision and ownership options

Retain the current on-demand derivation unless the avoided issuance work justifies
extra resident metadata and publication CPU. Compare workloads with and without
session persistence, new-session/refresh ratios, endpoint count and configuration
churn. No benchmark currently establishes that tradeoff.

Possible payloads include eight digest bytes or 16 ASCII hex bytes per endpoint;
these are candidate payload sizes, not measurements of total struct/Arc layout.
A String adds its own owner and heap cost. Inline digest bytes still require
rendering for the wire. Measure actual layout, padding, retained generations,
publication cost and request allocations before selecting a representation.

`ServiceDiscoveryCoordinator::materialize_section` creates canonical endpoint
wrappers. Placing derived data there charges all materialized endpoints even
when no route uses stickiness. Route-demand-dependent preparation would instead
introduce a publication dependency; it is not an incidental part of this task.
Do not add a process-global token cache or a second hashing implementation.
A prepared token must remain an immutable endpoint-owned value, never reusable
transport counter state shared across distinct endpoint proofs.

## Constraints and acceptance

- Preserve the canonical LB socket string, including IPv6 brackets and serving
  port. Do not switch to peer IP/port or change wire hash/truncation/hex format.
- Keep the same canonical derivation for map keys and response injection. Test
  unscoped equivalence and all optional-scope delimiter semantics explicitly.
- Do not use an unscoped prepared token for a scoped request. Preserve scoped
  bounded construction and its absence from the global Service affinity cache.
- Retain membership/generation revalidation, endpoint retirement and distinct
  proof ownership, including same-address replacements and retained old requests.
- Preserve the 1,024-entry map bound, iteration/collision behavior, route-local
  candidate order and health/strict-retry checks. Hash reuse grants no authority.
- Keep issuance-time expiry, captured TTL, Cookie/Header behavior, retry exclusion,
  101 exclusion and local-reply replacement semantics. This changes no threat
  model: affinity remains an unkeyed stickiness hint, not access control.
- Preserve missing selected-address and non-inet behavior. Do not infer a backend
  from a live generation after response processing has captured its attempt.

Review the representation and ownership design before implementation. Use existing
source-of-truth contracts in `skills/04-review/lb/regression-guards/sp-token-affinity-map-address-canonicalization.md`
and `skills/04-review/lb/intentional-design/crc32-session-affinity-not-a-security-boundary.md`.

Fresh baseline verification for this documentation audit passed 24 session-
persistence, seven LB runtime-state and four direct upstream-response-filter tests.
Formatting check, workspace check/Clippy and agent-doc validation passed with
existing unrelated warnings. No prepared-hash implementation, layout benchmark,
throughput measurement or live integration run was performed.
