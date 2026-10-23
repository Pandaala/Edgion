# Candidate provider-model ownership

Status: deferred performance/ownership decision for user review. No runtime change.
Source baseline: `091ad34b4`.

## Evidence

`ai/http/proxy.rs::resolve_candidate_models` borrows each candidate's retained
RuntimeAiBackend, checks streaming support, and copies the resolved canonical name
into AiCandidateModel alongside its pricing. Runtime resolution checks the exact
model map before the ordered wildcard-prefix list. Permitted unknown-model
passthrough instead copies the effective logical name and carries no pricing.
Zero-weight candidates can also receive a model copy; positive weight affects
the helper's final eligibility flag, not whether it constructs the model.

ReadyAiAttemptPlan owns these candidates and pins the captured backend snapshot;
each present candidate also retains its backend Arc. Ordinary retry selection
reuses candidate model ownership. Quota downgrade calls the resolver again,
clearing adaptive projections and disabling unknown-model passthrough before
admitting the replacement logical identity. Do not retain the previous model or
pricing merely because a backend pointer is unchanged.

`ai/runtime/dispatch.rs::prepare_ready_attempt` borrows the selected model for
request conversion/rewriting. It additionally clones the provider name into the
ChatToAnthropic response processor or the native Messages observer when those
paths are enabled. Those owners survive response callbacks and cannot borrow a
short-lived local selection reference. Native Messages can replace its initial
expected name with a bounded actual model from the selected start observation.
The separate AiRequestContext model copy is the logical quota/log identity, not
the provider model, and is outside this candidate's proposed unification.

## Alternatives and tradeoffs

- Retain Strings when bounded names and candidate counts make their simplicity
  preferable to new reference-counting or indirection.
- Share a name owner prepared in the retained backend generation; explicitly
  handle request-owned unknown-model passthrough and processor replacement with
  an observed actual name. Compare metadata, atomic clone/drop costs, cold
  construction and retained-generation memory, not just allocation count.
- Use an explicit resolved-model handle with its pinned backend owner, preserving
  exact resolution and pricing. Prove what lookup/indirection it requires and
  how wildcard selection and downgrade remain stable. Do not create unsafe
  self-referential candidate borrows or re-resolve against a newer global store.

No global string interner, second cache, new protocol/schema field or ownership
representation is selected. A plain reference cannot replace an owned field in
an independently retained plan/processor without an appropriate owning handle.
Sharing backend-owned names may save per-candidate copies but can also retain
owners longer; the existing plan already pins its generation, while processor
lifetime must be checked separately.

## Acceptance

Preserve exact-before-wildcard resolution and existing wildcard order, streaming
gates, canonical versus passthrough pricing, zero-weight candidates, invalid
shares and the boolean eligibility result. Preserve downgrade's replacement of
model and adaptive state, canonical rewrite requirements, and rewrite-before-quota
logical identity. Keep provider names, logical names and observed actual names
semantically distinct even when their bytes happen to match.

Verify multiple candidates, repeated provider names, aliases, wildcard matches,
unknown passthrough, disabled streaming, missing backends, downgrade success/failure,
provider/credential retries, old snapshots during reload, response cancellation
and actual-model replacement. Preserve conversion output bytes, usage attribution,
quota settlement and all existing memory/admission bounds. The usage-03 observed
iteration-string decision remains separate from initial candidate ownership.

Measure request construction and retries across name lengths and candidate counts,
including unknown models and response processor paths. Report allocations/copied
bytes, atomic operations, CPU, peak/retained memory and old-generation lifetime.
No measured throughput regression or saving is established by this source audit.
Freeze an exact implementation scope and obtain independent IMPL plan/final
reviews before any runtime changes. Add no tracing or metrics for this experiment.

## Original knowledge and verification

- [Compiled backend ownership](../../skills/04-review/ai-routing/regression-guards/precompiled-backend-lookup-keys.md)
- [Prepared admission ownership](../../skills/04-review/ai-routing/regression-guards/prepared-attempt-owns-admission-input.md)
- [Logical quota identity](../../skills/04-review/ai-routing/regression-guards/identity-model-quota-uses-requested-logical-name.md)
- [Observed iteration-model storage](usage-03-iteration-model-storage.md)
- [Structural selector/weight preparation](routing-08-model-route-weight-preparation.md)

Fresh unchanged-source runs passed 22 AI proxy, four model-router and 36 dispatch
tests (62 total). They exercise existing ownership and selection behavior, not a
proposed shared representation or all processor lifetime interleavings. No
prototype, allocation profile, benchmark, full suite or live integration ran.
