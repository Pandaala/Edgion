---
name: response-header-mutation-policy
description: Review configured and dynamic response mutations, protected fields, direct replies, and body-owner framing boundaries.
knowledge_type: regression-guards
---

# Response header mutation policy

## Conclusion and scope

Ordinary response mutations share `edgion_resources::response_header_policy`.
It reuses the canonical framing/hop-by-hop classifier and adds
`Proxy-Connection` and pseudo-header names. Comparisons are ASCII
case-insensitive. This response policy does not tighten request operations.

| Field class | Set/append/conditional writes | Remove/rename source | Rename destination | Body/transport owner |
|---|---|---|---|---|
| Ordinary end-to-end | Allow with existing syntax/value checks | Allow | Allow | Allow |
| Content-Length | Reject | Reject | Reject | Preserve verified length, remove for length changes, or compute exact local-body length |
| Canonical eight hop-by-hop names plus Proxy-Connection | Reject | Reject | Reject | Forwarding cleanup or protocol handshake |
| Pseudo-header | Reject | Reject | Reject | Status API or protocol codec |
| Received Connection nominee | New ordinary writes allowed after cleanup | Ordinary policy after cleanup | Ordinary policy after cleanup | Remove received nominated field and nominator |

Rejecting removal and rename-source operations is the approved Edgion product
boundary, not a claim that HTTP universally prohibits deleting these fields.
Both rename names are checked before either write; a rejected operation does
not roll back earlier successful operations.

## Producer and operation inventory

| Producer | Boundary and operation contract |
|---|---|
| Native HTTP/gRPC ResponseHeaderModifier; EdgionPlugins equivalent | Shared response validation at Controller/Gateway publication; set/add/remove order, all-entry validation and first-equivalent execution retained |
| ResponseRewrite | Shared validation of set/add/remove and both rename names; runtime rename/add/set/remove order retained |
| DSL and KeySet::ResponseHeader | Session string/typed set, append and remove guards; DSL retains existing error semantics |
| Proxy-Wasm response map and local reply | Session enforcement; replace-all replaces editable fields, preserves protected fields and valid repeated Set-Cookie; best-effort drain retained |
| ExtProc response, trailer and local reply | Shared response protection, including all PendingHeaderOp conditional variants; request protection remains separate; trailer-specific Host/gRPC checks remain |
| Mock and FaultInjection abort headers | Configured names validated; constructing a local body does not authorize configured framing overrides |
| Rate-limit custom names | Shared response protection plus existing feature-specific exclusions |
| RequestId | Existing stricter destination validator independently covers every protected name, plus routing, credential, and tracing exclusions |
| RegionRoute writeBack and response dye | Shared response validation; dye's request/business exclusions remain |
| ForwardAuth denial | Validated clientHeaders, including native HTTPRoute rule/backend and delegation source admission; filter every received Connection field's nominees, including automatic Content-Type; non-text Connection suppresses remote header copy; valid repeated selected values retained |
| PendingHeaderOp/local reply queue | Guard creation and application so direct queuing cannot bypass session setters |
| Direct finite local/error replies | Private ResponseHeader writes own exact length; gRPC trailers-only removes framing; queued ordinary mutations cannot replace it |

## Protocol and body-owner evidence

Native HTTPRoute ExternalAuth static admission uses the same ForwardAuth
validator as Gateway construction. Invalid rules are isolated before publication
or delegation; ordinary selected headers remain valid after an update. See the
[native ExternalAuth guard](gw-plugin-runtime-01-httproute-external-auth-invalid-plugins-never-sealed.md)
for `CAND-admission-001`, FileSystem/etcd reachability, and Kubernetes limitations.

| Path | Boundary |
|---|---|
| HTTP/1.1 final response | Ordinary mutations cannot choose length, transfer coding or reuse; forwarding sanitizer removes static and received dynamic hop fields |
| HTTP/2 and gRPC | Same ordinary mutation policy; no response TE exception; trailer/status validation stays protocol-specific |
| Interim 1xx / 101 | Ordinary response chains skip interim responses; 101 keeps transport-owned handshake fields |
| HEAD / no-body status / local reply | Existing bodyless and finite-body framing paths remain responsible |
| AI Anthropic / composite / usage | `anthropic/response.rs` always declares changed length; `dispatch.rs` aggregates processors; `usage.rs` uses quota + streaming + terminate-on-over-limit gates |
| Guardrail | Request-stage framing reservation precedes async processor installation; processor declares changed length; local replacement copies only fixed identity/security fields and computes body length |
| ExtProc streamed/trailers | Request-stage reservation in `ext_proc/plugin.rs`; overrides only reduce base modes; streamed processor declares changed length |
| Pingora compression / GrpcWeb | Pinned fork 518f727 directly removes Content-Length and sets stream framing through ResponseHeader, outside ordinary session guards |

`pg_upstream_response_body_filter::enforce_stream_processor_framing` removes
stale length at both response-header boundaries, including reservations for
later installations. Main response streaming remains unchanged. The independent
codec owners must never be routed through ordinary plugin setters merely to
reuse validation. See [sink ordering](../../http1/compatibility-constraints/response-body-sink-transform-order.md)
and [compression eligibility](../../http1/compatibility-constraints/compression-requires-known-response-length.md).

## Verification and re-evaluation

Verified on 2026-09-23:

- `cargo test --offline -p edgion-resources --lib response_header_policy` passed
  all 6 selected tests. The freshly built resource test binary also passed the
  ResponseRewrite, Mock, FaultInjection, dye, rate-limit, header-modifier, and
  RequestId protected-set regression groups.
- `cargo test --offline -p edgion-controller --lib response_header_policy` passed
  both tests; the same build passed all 9 existing header-modifier tests. These
  exercise rule rejection, valid siblings, recovery, and request-direction scope.
- The freshly built Gateway test binary passed policy, session-adapter, ExtProc,
  ResponseRewrite, Wasm, native modifier, plugin-runtime, error-reply, ForwardAuth,
  and schema/runtime parity tests. `cargo test --offline -p edgion-gateway --lib
  pg_response_filter` passed all 22 tests, including real in-memory H1/H2 delivery,
  HEAD, interim responses, 101, and protected queued fields on a finite local reply.
  H2 may omit Content-Length; when present on the tested empty reply it remains
  zero, and the actual body reaches clean EOS without injected connection fields.
- `framing_guard_applies_only_to_body_mutators`,
  `changes_body_length_tracks_every_precondition_the_terminate_gate_checks`, and
  `response_framing_reservation_remains_base_mode_derived_under_override` passed.
- Workspace formatting, `cargo check --workspace --all-targets`, and
  `cargo clippy --workspace --all-targets` passed. Agent-doc, SSA-force, metrics
  inventory, Pingora source, Gateway API transform-contract, gateway tracing, and
  English-only guards passed. Independent read-only code and knowledge review
  returned LGTM, including the body-owner and protocol exceptions.

Existing integration files remained frozen; full-workspace tests and deployed
integration scenarios were not run. In-memory codec tests and source inspection
do not establish conformance for every live protocol peer.

Re-evaluate when adding a producer, direct response writer, body transform,
late processor installation, new queued operation, or changing Pingora codecs.
Authority: repository HTTP body invariants and cross-cutting unification rules;
the exact ordinary-mutation policy was approved for this task.

## Resolution references

- `unify-response-header-mutation-policy` — fixed and verified; unified response policy with preserved body-owner framing.

## Prepared fixed values

Native header modifiers and ResponseRewrite share the runtime PreparedHeader
owner for fixed values. It parses HeaderValue and promotes shared byte storage
during construction, so the first request clone does not allocate value bytes.
Schema validation remains authoritative and still runs before preparation.
Session typed writers retain protected-header checks, queued/direct behavior
and header-name ownership.

Native modifiers keep their first-equivalent deduplication. ResponseRewrite
instead retains every configured duplicate and its rename/add/set/remove order.
Its rename values remain dynamic owned strings; only fixed add/set values use
the prepared owner. Invalid direct construction remains a logged no-op, and
publication continues to reject invalid configuration through existing gates.
Do not infer that header names, queues, response maps or plugin logs allocate
nothing from the zero-allocation prepared-value clone tests.
