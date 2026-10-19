# DynamicInternalUpstream routing-input ownership

Status: deferred optimization for user decision. No runtime change.
Source baseline: 9149f5b07.

## Evidence

DynamicInternalUpstream::run_request in
edgion-gateway/src/plugins/http/dynamic_internal_upstream/plugin.rs reads an owned
String through session.key_get. Optional regex extraction borrows a substring;
DynamicInternalUpstreamConfig::resolve_target then returns borrowed target fields.
In direct mode the target name borrows the input. In rules mode the target comes
from the first matching configured rule. The method currently expresses both
origins with one lifetime shared by self and routing_key.

After admission through the canonical compiled-backend matcher, the plugin copies
target fields into an owned InternalJumpPreset. On successful set_upstream_jump,
it still reads the resolved target to record the success outcome, composing
namespace/name when a namespace is present. Direct mode therefore keeps its
input owner through a mutable session operation under the present control flow.
A mechanical replacement with session_adapter::key_value would allow the input
to borrow that same session, conflicting with the later mutation.

This is not proof that the copy is inherently required by PluginSession, or that
borrowing is impossible. Rules-mode input is transient, and staged resolution can
separate its session lifetime from the configured target lifetime. Direct-mode
ownership can also be reorganized, but the jump and post-publication receipt need
an explicit ownership design. The shared key_value mechanism already borrows
ordinary sources and retains an owned fallback for RemoteIpAndPath; do not add a
second source reader or make a copying fallback slower to improve another source.

## Decision and alternatives

Decide whether to pursue a bounded staged-resolution change in a new IMPL plan.
The tradeoff is request-copy reduction against additional ownership/control-flow
representation and mode-dependent copy costs, not a proposed routing behavior
change.

- A rules-only scoped borrowing path could return a config-owned target after
  the input borrow ends. Keep extraction and error classification canonical;
  duplicating the entire rules/direct request pipeline would create drift risk.
- A shared staged result could distinguish a direct owned target from a borrowed
  configured target, plus typed missing/extraction/no-match outcomes. Its final
  publication stage must preserve the existing mutation and logging order.
- For an already-owned source, move or reuse its buffer where feasible. Copying
  every resolved direct target into a new String before also creating the jump
  could add a copy to a path that previously needed only the original input.
  Regex captures complicate reuse: retaining the original buffer, truncating or
  draining it, and copying only the capture have different work and capacity costs.
- An ownership receipt after jump publication is another possible boundary to
  assess; this task does not authorize a new public session API or moving success
  recording before jump acceptance. Keep request-retained jump fields owned.

No prototype or allocation/throughput measurement was run. A small source-edit
count alone does not establish the best representation. Conversely, the current
borrow conflict is not grounds to rule out a safe optimization.

## Acceptance for a selected implementation

- Use the canonical key_value source mapping, preserving first-header eligibility,
  cookie/query/context semantics, frozen request identities, and owned composite
  fallback. Do not import KeyAuth's source whitelist into this plugin.
- Preserve the gRPC early skip; missing or initially empty input; regex capture
  group selection; no capture versus an empty successful capture; and the current
  raw-input fallback when extract is present but compiled_regex is absent.
- Preserve direct targeting, first matching rule, duplicate-rule precedence,
  no-match behavior and exact target name/optional namespace bytes.
- Keep missing, regex-failure and no-match responses/outcomes distinct. Retain
  no-route and no-backend-refs pass-through before onInvalid policy. Use the same
  compiled name matcher, reference denial and slot validation before jump publish.
- Preserve jump-lock ordering and success-only outcome recording, including the
  existing namespace/name format and rejection diagnostics. No new tracing,
  metrics, diagnostic entries or changes to sensitive-data policy are implied.
- End session borrows before mutable calls; retain enough independent ownership
  for the jump and receipt without unsafe lifetime extensions or whole-session
  clones. Do not change selected-generation admission or later upstream ownership.
- Compare allocations, copied bytes and retained capacity for direct/rules modes,
  borrowed/owned sources, with/without regex, short captures from long inputs,
  missing/empty values, unmatched rules, denied targets and locked jumps. Account
  for output construction and receipt formatting, not just the initial read.
- Differential tests must pin response bodies, error tags, outcomes, jump fields
  and ordering. Keep tests against real borrowed header/query/context adapters
  where needed; do not preserve hot-path copies for mock convenience.

## Original knowledge and verification

Relevant original contracts:

- skills/04-review/routing/regression-guards/denied-named-jump-fails-closed.md
- skills/04-review/key-auth/intentional-design/extract-first-not-shared.md
- skills/04-review/cpu-memory/regression-guards/response-condition-live-input-borrowing.md

Fresh serial verification of unchanged code passed 83 network-free tests:
10 DynamicInternalUpstream, six resource configuration and 67 condition tests.
The condition suite provides shared borrowed-reader coverage; it is not proof
that the proposed new DynamicInternalUpstream ownership shape is correct.
Formatting, workspace check/Clippy and document/diff validation passed. No live
transport, full workspace test suite or ownership prototype ran.
