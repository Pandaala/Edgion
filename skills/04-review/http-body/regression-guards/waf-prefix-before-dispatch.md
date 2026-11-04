---
name: waf-prefix-before-dispatch
description: Review native WAF prefix admission, shared request-body capture, and first-attempt forwarding.
knowledge_type: regression-guards
---

# WAF prefix admission before backend dispatch

## Conclusion and scope

`WafConfig.request_body.inspection` selects default `full` or explicit `prefix`.
Prefix mode evaluates a configurable raw payload window before upstream selection
or connection: `prefixSize` defaults to 64 KiB and is bounded to 1 byte through
1 MiB. The shared size parser resolves it at configuration load, never per request. Admission says only that no eligible rule blocked that window.
It does not certify the remaining body. The physical request-body cap remains
independent and may fail after upstream exposure.

The user amended the original task's native-retry-buffer-only proposal: reuse
the canonical registered `RequestBodyBuffer` seam and Edgion `BodyStorage`.
Explicit-prefix WAF-only capture capacity equals its inspection window and stays entirely in
memory, including 1 MiB windows. This user-requested policy supersedes the
earlier spill-threshold proposal. Full capture retains its memory/disk policy;
`prefixSize` with `full` is rejected.
A pre-existing complete cache supplies its current logical prefix directly;
subsequent mutations are refused at the shared snapshot publication boundary.

## Rationale and evidence

Previously `WafPlugin::run_request` installed a raw streaming handler and returned
`GoodNext`. Pingora wrote upstream headers before invoking the body callbacks,
so a late rule match could not withdraw headers or previously forwarded bytes.
The existing complete-capture lane already supported streaming writes and bounded
replay, but required real EOF before any replay. Partial capture was not a valid
complete replay source.

The new prefix path seals a bounded storage object without fabricating transport
EOS. Pingora retains first-attempt delivery state independently of transport
completion, drains the prefix and any bounded overrun once, then resumes live
reads. Cancellation poisons capture. Registered prefix and complete sources
remain mutually exclusive; a prefix view never becomes `RequestBodyState`'s
complete cache. A later complete-body consumer fails closed if no complete cache
was present before prefix capture.

Relevant source: `plugins/http/waf/mod.rs`,
`plugins/runtime/session_adapter.rs::read_request_body_prefix`,
`request_body/storage.rs::BodyStorageView::publish_arc`, and the Pingora fork's
registered request-body buffer and request relay contracts.

## Alternatives and limits

- The shared compiler checks prefix eligibility across the generation, not merely
  the existing `StreamingRaw` execution class. Positive raw matches are supported;
  absence/EOS-dependent, structured, chained, and control-flow semantics require
  `full`. Later phases cannot depend on an unavailable full request body.
- Prefix completion is explicit (`NotRequired`, `EndOfStream`, `WindowFilled`);
  the engine does not publish an empty or truncated body as a complete document.
- Full mode publishes only complete documents. When its effective inspection
  window is below the hard cap and the selected WAF is the sole body consumer,
  it may use the canonical native N+1 capture lane. A policy cutoff discards
  provisional JSON/XML state and stops all remaining WAF phases; it does not
  publish a partial document. Other complete consumers retain complete capture.
- Phase two alone does not imply body consumption. Compiler target and macro
  dependency analysis is shared by prefix eligibility, execution, and capture
  planning. Body-independent phase-two rules use ordinary evaluation and may
  run with `NotRequired` prefix completion; mixed rules retain schedule order.
  Header/URI and `ARGS_GET` / `ARGS_GET_NAMES` consumers do not capture a body in
  either mode. `ARGS`, `ARGS_NAMES`, processor metadata, and later-phase body
  dependencies remain conservatively body-dependent. Prefix restrictions on
  macros, chains, and control/state actions remain unchanged.
- Native prefix capture disables retry, including short bodies. Reusing an already
  complete cache keeps its existing replay policy.
- Transport overrun is distinct from cached inspection bytes. Its backing must be
  bounded and released, not hidden behind a small `Bytes::slice` length.
- Prefix physical limits count transport bytes once, including pre-read/overrun,
  and remain active when no per-chunk body plugins are installed. Ordinary HTTP
  also enforces the pinned physical cap without WAF or storage capture. Original
  CONNECT and canonical gRPC are excluded, and only an accepted native upgrade
  disables admission for tunnel bytes. Replay never double-counts. Selected-WAF optional
  JSON/XML hard limits additionally remain active without body-dependent rules.
- `detectionOnly` suppresses rule interventions, not transport or storage failures.

## Verification and re-evaluation

### Full JSON capture-chunk inspection

The effective JSON processor after Phase 1 selects the native fused checker;
configuration defaults provide independent lexical/structure/result budgets.
Unsupported demands fail admission without a DOM fallback. The conservative
reserved StreamingStructured classifier is not promoted by this entry.

Fresh capture feeds borrowed chunks from the canonical downstream loop. Cached
sources feed bounded immutable snapshot chunks after pre-IO size admission.
Transport bodyless knowledge comes from the immutable request snapshot. Completion
requires real transport/reader None and exact length parity, followed by the
original complete-source BodyAccounting prelude. Reached length alone is not EOF.
Inspection failures latch and stop grammar work while bounded capture continues;
transport/storage/capture and complete-source admission retain their priority.

The scoped guard seals selected facts and then the canonical Phase 2 schedule
runs before GoodNext. Canonical BodyStorage preserves forwarding/retry; JSON-only
spill uses no complete materialization. Forced raw visibility or DetectionOnly
syntax fallback can require one canonical read. Body version freezes for inspection;
reader errors and cancellation drop provisional state and its symmetric charge.

The existing runtime gauge temporarily includes known current parser/result
capacities plus logical TX charge; it hands off once to installed transaction
accounting. It is not RSS or total physical memory. JSON inspected-byte deltas
come from lexer-admitted source; Full non-JSON records submitted bytes once and
prefix retains its current accounting. No replay/per-rule byte multiplication.
The phase execution timer still excludes body capture and parser publication.

Completed execution plans were removed during task cleanup. Current structured
inspection contracts live in the [JSON guard](../../waf/regression-guards/fused-json-inspection.md)
and [XML guard](../../waf/regression-guards/native-xml-inspection.md).
Gateway performance and Kubernetes admission/reload require fresh verification
using the [testing guide](../../../05-testing/01-integration-testing.md). The
historical task records are absent from the current tree; their removal does
not establish completed validation.
Historical prefix evidence below does not prove current JSON timing or wire gates.

Prefix rules execute lazily in the canonical phase schedule. An enforced deny
ends evaluation immediately; unreachable rules cannot replace that decision
with a work-budget error. Detection-only and nonmatching rules still consume
the configured evaluation/work budgets. Untransformed bytes are borrowed, while
the transformation output bound and prefix operator-work accounting remain
unchanged. No generation-sized prefix match table is retained. Regression tests
cover a 1 MiB prefix with 33 rules, first-rule denial, actual budget exhaustion,
detection-only findings, and the output bound on borrowed bytes.

Body-dependency regression tests cover prefix configuration acceptance for
body-independent phase-two targets, zero Gateway body reads in both modes,
`NotRequired` phase-two denial, and both orderings of mixed header/raw rules.
These unit tests complement the Gateway wire/hot-reload verification below.

Before the configurable-window amendment, local verification passed with an external temporary Cargo patch to the sibling
Pingora branch `feat/waf-request-body-prefix`: workspace check and Clippy,
formatting, Gateway WAF tests (38), resource WAF tests (9), and CRD coverage (19).
Independent review approved that baseline and the subsequent configurable-window
amendment. Fresh verification of the amendment passed workspace check/Clippy,
formatting, 7 engine prefix-related tests, 10 resource WAF tests, 39 Gateway
WAF-related tests, 19 CRD coverage tests, and 6 plugin CRD type tests. The 1 MiB
hot-reload wire matrix was compiled but had not run at that historical stage.
Documentation, SSA, metrics inventory, and Gateway API transform guards passed. Pingora's request-body seam wire matrix passed all 65 cases, with
focused prefix, cancellation, and retry-refusal tests also passing.

The Gateway WafDynamic wire/hot-reload matrix passed locally on 2026-09-23
using the shared Controller/Gateway topology. It covers all four H1/H2 protocol
combinations, N-1/N/N+1 body lengths and digests, prefix boundary matches, zero
backend exposure on denial, chunked upload with local 100 Continue, prefix/full
switching, a 1 MiB prefix, invalid-generation retention, detection-only, and
delete/recovery. The wire fixture must use a domain URL with an explicit local
address mapping so H2 authority and Host agree; its H1 backend version marker
is `1.1`. Both fixture corrections retain the strict forwarding assertions.
The historical prefix/discard work is integrated into the current supported
Edgion Pingora fork. Cargo.toml selects `edgion_v0.9.1`; Cargo.lock pins the
canonical Git revision. The earlier local `edgion_v5` publication note is
superseded and is not an outstanding WAF task.
This historical wire result does not prove trailer preservation.
Re-evaluate when adding rule shapes, changing the prefix window, enabling prefix
retry, changing body source ownership, or allowing post-inspection body mutation.

## Optional format body admission (2026-10-09)

The selected WAF may install `maxJsonBodySize` / `maxXmlBodySize` after Phase 1
processor selection, independently of prefix/full coverage and body dependency.
DetectionOnly does not disable an explicit admission ceiling. The effective
limit is the tighter Gateway/format bound. Installation checks known native and
logical sizes and freezes canonical body mutation, including later snapshot
publication; mutating stream handlers conflict in both installation orders.
The native relay guard survives transaction removal and counts received bytes,
not replayed bytes. It retains the existing already-selected-response contract.

Fresh unit, workspace check/Clippy and CRD checks passed. The new WafDynamic
format phases passed in the shared topology across all four H1/H2 combinations,
including N-1/N/N+1, unknown lengths and paused post-dispatch overflow with
body-independent DetectionOnly. The first full rerun subsequently failed during
fixture deletion because its cleanup list retained an old invalid resource name;
that list is corrected. Fresh full WafDynamic then passed, including resource
cleanup, under `tmp/integration_testing/testing_20261009_225253` (exit 0).
Those historical results alone did not establish the subsequent
default-full/storage-cutoff policy; its fresh acceptance is recorded below.

## Resolution references

- `waf-request-verdict-before-forwarding` — confirmed missing admission contract;
  implemented through the canonical admission seam; subsequent policy acceptance
  and its boundaries are recorded below.


### Explicit full-mode policy cutoff

`json_capture::InspectionPolicy` separates the hard source cap from the soft
inspection window. The cached reader admits total logical length before a short
read and retains real EOF/parity checking for complete sources. Fresh WAF-only
capture uses the same canonical BodyStorage with native N+1 proof and replay.
The sealed view remains available through parser completion: DetectionOnly can
require raw fallback only after feed-time or EOF-only syntax failure. Materialize
that source only after completion requires it. Never eagerly copy a JSON-only
source or expose partial storage as a complete request snapshot.

`stop_by_policy` does not feed EOF, publish results or poison the transaction as
abandoned. Genuine latched and deferred syntax errors remain failures. The
request context claims all remaining phases, records bounded inspection-stop
metadata and retains the independently installed hard body guard. A policy stop
must not be labeled a complete clean result. Re-evaluate this contract whenever
consumer composition, capture admission, parser fallback or phase ownership changes.

## Unified policy acceptance (2026-10-10)

Default Full now shares one effective source-byte ceiling across transaction
BodyLimits, JSON lexical accounting and XML parser accounting. It starts from
the pinned EGC physical cap and may narrow before publication. The removed
operator `json.lexical.maxSourceBytes` has no alias. Token, decoded, depth,
heap, node and work limits remain independent; a larger source allowance does
not promise successful inspection of an arbitrarily complex document.

Cross-resource maxInspectSize compatibility is checked on the actual pinned
request pair before WAF initiates body reads/inspection, including when another
consumer already captured a complete snapshot. An incompatible pair fails with
the fixed runtime configuration error; it is never clamped or treated as a
policy cutoff. No publication-time cross-resource diagnostic is implemented.
One WAF generation may be attached to independently updated EGC scopes.

The full shared WafDynamic run at `testing_20261010_001631` passed all four H1/H2
directions, format and inspection boundaries, disk N+1 replay digests, dynamic
configuration, ordinary HTTP admission with capture disabled, and early-response
preservation; 16 checks passed, zero failed, two setup checks were expected skips.
A native callback test separately proves a real overflow after response selection
keeps the selected 200; wire early responses may cancel before tail reads.
Source-byte tests admit whitespace/comment-heavy JSON/XML above 16 MiB under the
default 32 MiB cap while independent zero-work budgets still reject.

Supplemental acceptance exercised canonical storage deadlines/admission/poisoning,
reader EOF/parity, cancellation, replay and owner-capacity atomicity, plus engine
partial-document stop, duplicate-key and sticky-error regressions. These are
per-request bounds, not process RSS/disk quotas. The process-wide byte ledger
remains deferred. No new trailer forwarding contract or Kubernetes cluster test
is established by the local shared topology; existing trailer/discard and
protocol-specific replay contracts remain unchanged.


## Dormant streaming projection decision

User-directed value review at `895a8efd1` closed waf-16 without implementation.
Current production WafPlugin activates prefix/full evaluation, not
start_request_body_stream or enable_request_body_streaming. The default-false
context flag gates run_waf_body_stream before execution. Optimizing cumulative
hit projection in the dormant leaf raw-stream API therefore saves no current
Gateway traffic work and would add scratch/delta state and layered error rules.

Retain the current cumulative native API. Historical iterator/projection tests
remain evidence only for that API, not production reachability. Reopen only with
a separately authorized demonstrated production consumer, then reassess hit/chunk
cost and preserve bounded admission, cumulative semantics and the pre-dispatch
security boundary. Closing this proposal authorizes neither API removal nor
post-dispatch WAF reactivation. Active prefix/backend scan proposals are separate.
