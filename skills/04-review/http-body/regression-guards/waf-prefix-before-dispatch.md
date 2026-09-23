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
- Full mode uses complete `BodyStorage` capture even for raw phrase rules.
- Native prefix capture disables retry, including short bodies. Reusing an already
  complete cache keeps its existing replay policy.
- Transport overrun is distinct from cached inspection bytes. Its backing must be
  bounded and released, not hidden behind a small `Bytes::slice` length.
- Physical limits count transport bytes once, including pre-read/overrun, and
  remain active when no body plugins are installed. Replay never double-counts.
- `detectionOnly` suppresses rule interventions, not transport or storage failures.

## Verification and re-evaluation

Before the configurable-window amendment, local verification passed with an external temporary Cargo patch to the sibling
Pingora branch `feat/waf-request-body-prefix`: workspace check and Clippy,
formatting, Gateway WAF tests (38), resource WAF tests (9), and CRD coverage (19).
Independent review approved that baseline and the subsequent configurable-window
amendment. Fresh verification of the amendment passed workspace check/Clippy,
formatting, 7 engine prefix-related tests, 10 resource WAF tests, 39 Gateway
WAF-related tests, 19 CRD coverage tests, and 6 plugin CRD type tests. The 1 MiB
hot-reload wire matrix is compiled but remains unrun with the shared topology.
Documentation, SSA, metrics inventory, and Gateway API transform guards passed. Pingora's request-body seam wire matrix passed all 65 cases, with
focused prefix, cancellation, and retry-refusal tests also passing.

The Gateway WafDynamic wire/hot-reload matrix is implemented and compiled but
has not run: switching the existing shared test topology requires user approval.
Publishing the fork through `edgion_v4` and updating the final lockfile remain
pending. The repository lockfile retains the original published revision; its
source-policy guard passes only for that baseline, not the new local APIs.
The active task stays open until these integration and publication gates pass.
Re-evaluate when adding rule shapes, changing the prefix window, enabling prefix
retry, changing body source ownership, or allowing post-inspection body mutation.

## Resolution references

- `waf-request-verdict-before-forwarding` — confirmed missing admission contract;
  implementation and verification tracked in the active task until closure gates pass.
