---
name: request-body-read-contract-ownership
description: Use when reviewing ownership of request-body read errors, borrowed consumers and immutable snapshot readers.
knowledge_type: intentional-design
---

# Request-body read contracts belong to the body subsystem

## Conclusion and scope

The Gateway request-body subsystem owns `BodyReadError`, `BodyReadResult`,
`RequestBodyChunkConsumer`, and `RequestBodySnapshotReader` in
`edgion-gateway/src/request_body/contracts.rs`, publicly exported by
`request_body/mod.rs`. PluginSession and feature consumers import that owner.
The choice unifies the definition of storage/read capabilities with their
implementation owner. It does not reduce consumer fanout or claim runtime gains.

## Rationale and evidence

ARCH-002 identified fundamental body contracts defined in
`plugins/runtime/traits/session.rs` even though storage, state, operations and
scanners implemented or consumed them. The existing resident-slice capability
illustrates the maintenance cost: its definition sat in plugin session while
`request_body/storage.rs` implemented it. A future snapshot capability now starts
beside storage instead of changing the common plugin-session declaration owner.
The approved move preserves all eight error variants, their Display/Error
implementations, trait signatures, Send bounds, async_trait and default bodies.
See [plugin architecture](../../../01-architecture/02-gateway/05-plugin-system.md#request-body-read-contract-ownership)
and [extension guidance](../../../01-architecture/02-gateway/12-edgion-plugin-dev.md#complete-request-body-readers-and-borrowed-consumers).

## Alternatives and limits

Keeping the original placement was the simpler alternative and remains a
reasonable tradeoff if the modest ownership benefit ceases to justify migration.
Splitting another file within plugin runtime would preserve the wrong dependency
direction. No legacy exports or compatibility layer are retained under the
project's pre-release change discipline.

PluginSession retains policy, stage and transport methods, including
`RequestBodyInspectionCapture`; the observer retains PluginLog integration.
Complete independence from plugin runtime is not claimed. This decision adds no
new API, cache, dispatch, allocation, lock, WAF feature or runtime behavior.
Immutable generations, replay cursors, reader backing lifetime, isolation budgets,
actual EOF/parity, physical/logical admission, prefix/stage gates, retries,
cancellation and storage deadlines continue under their existing owners.

## Verification and re-evaluation

Fresh verification on 2026-10-11 at source baseline ef69a780 plus this scoped
change passed `cargo check --workspace --all-targets`,
`cargo clippy --workspace --all-targets` (zero warnings), formatting, diff hygiene,
and the agent-doc, SSA force, metrics inventory, Pingora source policy, Gateway
API transform, unit-test layout and Gateway no-tracing guards.

Eight filtered `cargo test -p edgion-gateway --lib <filter>` runs passed 275 tests,
with zero failures, ignored tests or empty filters: request_body (110),
session_adapter::tests (49), body_gate::tests (27), WAF (50), RequestMirror (27),
DSL builtins body_tests (6), opaque_body_stream_tests (4), and context body-cap
regressions (2). The declaration span was byte-identical after relocation;
consumer/test bodies, names, gates and paths were unchanged after import/path and
rustfmt normalization. No full `cargo test --all` or integration topology ran.
This establishes the scoped migration, not every body subsystem behavior.
Independent final review remains required before task closure.

Re-evaluate if contracts acquire stage/policy/transport dependencies, a new body
owner or whole-response API is proposed, snapshot generation/backing/isolation
semantics change, or consumers start bypassing adapter admission. This accepted
placement does not suppress defects in those runtime contracts.

## Resolution references

- ARCH-002: request-body contract ownership; approved bounded AIMPL-C proposal
  on 2026-10-11. Original finding remains a source-review input snapshot.
