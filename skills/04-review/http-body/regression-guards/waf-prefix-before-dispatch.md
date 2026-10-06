---
name: waf-prefix-before-dispatch
description: Review native WAF prefix admission, shared request-body capture, and first-attempt forwarding.
knowledge_type: regression-guards
---

# WAF prefix admission before backend dispatch

## Conclusion and scope

`WafConfig.request_body.inspection` selects default `prefix` or explicit `full`.
Prefix mode evaluates a configurable raw payload window before upstream selection
or connection: `prefixSize` defaults to 64 KiB and is bounded to 1 byte through
1 MiB. The shared size parser resolves it at configuration load, never per request. Admission says only that no eligible rule blocked that window.
It does not certify the remaining body. The physical request-body cap remains
independent and may fail after upstream exposure.

The user amended the original task's native-retry-buffer-only proposal: reuse
the canonical registered `RequestBodyBuffer` seam and Edgion `BodyStorage`.
WAF-only capture capacity equals its inspection window and stays entirely in
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
- Full mode uses complete `BodyStorage` capture for body consumers, including raw phrase rules.
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
- Physical limits count transport bytes once, including pre-read/overrun, and
  remain active when no body plugins are installed. Replay never double-counts.
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

Current validation is tracked in
[plan 16](../../../../tasks/waf-todo/16-json-capture-wiring-and-closure.md).
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
The prefix implementation and discard correction are consolidated into one
commit on local `edgion_v5`. Edgion's manifest and lockfile select that branch
and commit through the canonical Git source, without a path override. Remote
publication remains pending under the user's no-push instruction.
The active task stays open until remote publication and clean remote-source
verification pass. This wire result does not prove trailer preservation.
Re-evaluate when adding rule shapes, changing the prefix window, enabling prefix
retry, changing body source ownership, or allowing post-inspection body mutation.

## Resolution references

- `waf-request-verdict-before-forwarding` — confirmed missing admission contract;
  implementation and verification tracked in the active task until closure gates pass.
