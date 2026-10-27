---
name: execution-time-authorization-context
description: Use when reviewing ForwardAuth request identity, metadata precedence, and auth-hop defaults after rewrites.
knowledge_type: intentional-design
---

# ForwardAuth Authorizes Execution-Time Context

## Conclusion and scope

ForwardAuth and inline HTTPRoute ExternalAuth share execution-time authorization
context. The automatic X-Forwarded-Host/Uri/Method/Query headers read current Host,
normalized path, method and raw query string. Ordinary forwarded headers/cookies
and `forwardBody` also observe current values. `forwardBody` reads the current
committed BodyStorage generation; there is no separate entry-body copy.

Auth-hop method/path are independent: omission reuses frozen entry values;
explicit templates such as `${method}` and `${path}` read current values.
The hop's own query arguments come from `request.args`.

Ordinary headers are built through forwardAll, origin, then custom. Automatic
metadata is inserted last, replacing same-name values even with forwardAll false.
Required origin fields must still be present before final injection. Missing Host
or query produces empty metadata values. X-Forwarded-Uri contains no query.

## Rationale and evidence

T03-A identified a documentation defect: the full user guides and runtime/schema
comments promised original request metadata despite execution-time reads.
On 2026-10-13 the user accepted correcting this description and adding boundary
regressions while preserving runtime behavior. This is an explicit scoped
contract choice, not proof of correctness inferred from existing code.

The benefit is consistent authorization of the already-adapted current request.
The cost is that operators must place auth before relevant rewrites when they
intend to authorize entry identity. Auth metadata and auth-hop defaults can refer
to different views; the documentation now distinguishes them explicitly.

Sources:

- [Entry/current development contract](../../../01-architecture/02-gateway/12-edgion-plugin-dev.md#entry-and-current-request-identity).
- `edgion-gateway/src/plugins/http/forward_auth/plugin.rs`: run_request reads
  current committed body and context_headers selects current metadata.
- `edgion-gateway/src/plugins/http/common/outbound/request.rs`: build_request
  selects entry defaults and evaluates explicit templates.
- `edgion-gateway/src/plugins/http/common/outbound/call.rs`: finalize_request
  inserts caller metadata, applies body override and synchronizes framing.
- `edgion-gateway/src/plugins/runtime/plugin_runtime.rs`: ExternalAuth uses
  ForwardAuth::from_route and the same request filter.
- [English guide](../../../../docs/en/user-guide/edgion-plugins/forward-auth.md)
  and [Chinese guide](../../../../docs/zh-CN/user-guide/edgion-plugins/forward-auth.md).

## Alternatives and limits

Switching only method metadata to entry identity would mix identity views.
A complete entry-context implementation would require separately reviewed scope
and an explicit product requirement; no header/body snapshot is authorized here.
There is no new metadata opt-out. Unconditional injection was already documented
and is not itself the defect.

This choice does not seal the request against later trusted rewrites, guarantee
authorization of the final upstream message, or establish an authorization bypass.
General outbound origin timing remains outside T03-A.

Preserve the existing [body-source mutual exclusion](../regression-guards/request-body-sources-mutually-exclusive.md)
and [outbound framing ownership](../regression-guards/outbound-subrequest-framing-owned-by-hop.md).
Body bounds, failure policy, retry policy, redaction and observability are unchanged.

## Verification and re-evaluation

Added focused regression cases cover current metadata with and without rewrites, spoofed
inbound/origin/custom values, forwardAll false/true, entry defaults and current
or fixed templates. ForwardAuth body-read wiring, real adapter committed-body
reads in memory/spill, and final bytes/framing have assertions at their owning seams
without starting a network server. These tests do not constitute live topology
or cross-process ExternalAuth coverage.

Fresh verification on 2026-10-13:

- `cargo test -p edgion-resources --lib resources::edgion_plugins::plugin_configs::forward_auth::tests`:
  12 passed, including body-source mutual exclusion and content-type conflicts.
- Agent-doc validation, SSA force, metrics inventory, Pingora source policy,
  Gateway API transform contract and unit-test layout guards passed.
- Direct rustfmt checks on changed Rust files and `git diff --check` passed.
- Workspace format/check/clippy and Gateway ForwardAuth tests are blocked by
  existing conflict markers in HEAD: plugin_runtime.rs, session_adapter.rs and
  pg_upstream_response_body_filter.rs. The compiler/formatter first stops at
  plugin_runtime.rs. The new Gateway tests have not executed and are not claimed
  to pass. No live integration topology or full workspace tests ran.

The active T03-A task remains pending until those gates can pass. This page
records the accepted contract and implementation, not a verified task closure.

Re-evaluate if the product requires entry-object authorization regardless of
plugin ordering, metadata injection becomes configurable, request mutation
ownership changes, or inline ExternalAuth stops sharing ForwardAuth.

## Resolution references

- T03-A / T03: original-request description mismatch; corrected documentation
  and accepted execution-time context, with boundary regression coverage.

## Borrowed current query metadata

ForwardAuth context_headers reads request_query directly alongside the other
current-session metadata. Both production query accessors read the same logical
current Query owner; this removes the get_query temporary String without switching
to frozen entry-query data. Body acquisition still finishes before these reads.

The borrowed array and shared session remain valid through the outbound call.
Final request construction owns its HeaderValues before sending/retrying, and
session mutation resumes after the call result. This does not remove outbound
header storage or change metadata precedence, failure handling or retry behavior.
The existing metadata test covers current/rewritten identity, absent and present
empty queries, duplicate/encoded raw query text and nonempty query slice identity.
These checks concern this ownership boundary, not live authorization topology.
