# Evaluate AI request-baseline rebuilding cost

Status: deferred optimization investigation for user decision. No runtime change.
Inspected baseline: `d4d716e21`.

## Evidence and decision

`edgion-gateway/src/ai/context.rs::AiRequestBaseline` owns a complete Pingora
RequestHeader and optional DeferredCredentialMutations. Capture clones them once;
restore clones them into mutable attempt state without consuming the baseline.
`ai/runtime/dispatch.rs::prepare_ready_attempt` restores before target selection
and refreshes normalized path after restoration. Initial capture occurs after
body preparation and before backend header modifiers and provider preparation.
Later candidates reuse the same post-rule live/deferred baseline.

This is required isolation, not redundant state: previous backend/provider writes
and deferred protocol or credential fields must not leak into another candidate.
The scope includes predispatch skips and later AI attempts. It is not the ordinary
endpoint-retry policy, which retains its logical backend selection.

The pinned fork `1aaa45cc70430bc782990bc20ddee3fef36d6b99` implements RequestHeader
Clone through clone_req_parts (method, URI, version, header map and extensions),
plus header_name_map, raw_target and send_end_stream. It has no optimized
clone_from override: replacing assignment with clone_from alone does not establish
capacity reuse. Rebuilding through request Parts loses raw-target/case information
unless separately restored. Container clones do not imply every value payload is
deep-copied; HeaderValue/Bytes/Arc ownership must be measured independently.
The baseline owns no body snapshot; request bodies retain their existing canonical
BodyStorage and AI plan ownership.

Choose whether measured workloads justify a canonical capacity-reusing restore or
an explicitly tracked mutation/rebuild design, or retain full cloning. A shared
Arc baseline alone does not avoid creating independently mutable attempt headers.
Consuming/moving the sole baseline cannot support repeated restores. A selective
header-copy shortcut cannot assume providers only change credentials: backend
modifiers, provider endpoint/method/URI/protocol fields and deferred state all
participate. Do not add an ad hoc second request-header owner or bypass Pingora's
case/raw-target maintenance APIs.

The tradeoff includes initial capture cost, per-candidate rebuild cost, retained
map/vector capacity, old-generation references, attempt count, extension clone
behavior and the request lifetime. Capacity reuse can retain a larger prior
attempt's allocations; mutation tracking adds bookkeeping even to single-attempt
requests. No candidate, net heap reduction or throughput gain is established.

## Acceptance for a selected implementation

- Freeze an exact allow-list and independently review the owner/interface design.
  Preserve capture-once timing and restore before selection, including failures
  after restoration, backend skips, missing/exhausted targets and retries.
- Preserve method, version, URI, original raw target, name-case map, all repeated
  fields, value bytes and sensitivity, extensions and end-stream state. Prove
  original HTTP/1 wire output and relevant HTTP/2 request state against the current
  clone behavior; direct access to only the public value map is insufficient.
- Preserve deferred winning targets, values, sensitivity and validity deadlines;
  remove prior candidate-only mutations and restore the captured baseline on every
  candidate. Keep final Connection nomination and expiry validation before upstream
  materialization. Names and values must not enter diagnostics or logs.
- Keep normalized-path refresh and canonical URI access synchronized. Preserve
  request-body generation checks, canonical body ownership, provider isolation,
  existing attempt admission/reservations and scheduler/dispatch counter ordering.
  This task grants no new retry, cancellation or failure policy.
- Preserve the allocation-free absent-baseline path for ordinary HTTP. Measure
  initial capture, first preparation, repeated same-provider attempts, provider
  changes, predispatch failures, short/large header sets, duplicate fields and
  deferred mutations. Account for peak/retained heap through cancellation and
  context teardown, not only allocations inside a single restore call.
- Extend discriminating context/dispatch tests for the chosen implementation;
  retain existing tests and frozen integration fixtures. Perform independent IMPL
  plan/final review and project checks. Baseline tests alone do not prove a new
  restoration representation equivalent or faster.

Original authority:
- [AI failover runtime boundaries](../../skills/04-review/ai-routing/regression-guards/ai-failover-tests-must-cross-runtime-boundaries.md)
- [Ordinary retry keeps logical selection](../../skills/04-review/routing/review-evidence/retry-keeps-logical-backend-selection.md)
- [Deferred credential nomination guard](../../skills/04-review/security/regression-guards/deferred-credential-connection-nomination.md)

Fresh current-code tests passed nine AI context and 36 AI dispatch cases, including
capture-once behavior and deferred protocol restoration across skips/retries.
No candidate implementation, benchmark, full workspace test suite or live
integration ran. These results establish baseline behavior, not a performance
improvement or complete coverage of every RequestHeader field.
Formatting, workspace all-target check/Clippy, agent-doc validation and diff
hygiene passed with existing unrelated Clippy warnings.
