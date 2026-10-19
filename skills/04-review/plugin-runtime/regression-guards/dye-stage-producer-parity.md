---
name: dye-stage-producer-parity
description: Review entry dye capability against configured stage, producer paths, failure policy, and response-header commit timing.
knowledge_type: regression-guards
---

# Dye stage and producer parity

## Conclusion and scope

D0083 identified real stage/capability drift: Wasm response-body entries accepted
failure dye without any producer or mutable response header; AiProxy produced
undeclared outcomes; ExtProc produced undeclared Success and late failure dye.
The shared capability now resolves the configured stage and static policy. The
user chose complete repair while keeping AiProxy dye unsupported and retaining
only ExtProc header dye that can be rendered before commit.

The canonical contract and complete producer inventory live in
[entry-level dye](../../../01-architecture/02-gateway/entry-level-dye.md).
This replaces the absent `2026-07-20-entry-level-dye-suite-design.md` authority;
it does not claim to recover that missing document's contents.

## Rationale and evidence

- `edgion-resources/.../edgion_plugins/entry.rs` passes a `DyeStage` to the single
  `EdgionPlugin::dye_capabilities` contract. Conditions and templates still use
  existing shared implementations.
- `plugin_runtime.rs` reports a schema dye error and omits its compiled plan;
  it does not change the existing plugin activation/severity policy.
- `session_adapter.rs::record_outcome` queues request-stage response operations
  or writes a bound response header. Post-commit ExtProc body/trailer records
  cannot use that lifecycle and were removed, along with their unused plan pin.
- AiProxy quota decisions keep their existing logging and denial behavior; the
  removed `record_outcome` calls were not configurable public behavior.
- Canary's internal kill switch is different from entry disable. The frozen
  `20_EdgionPlugins_default_canary-kill-switch-strip.yaml` and its client scenario
  require the active entry to retain anti-spoof stripping while selection pauses.

Compiled dye rules prepare outcome membership as a private bit mask using an
exhaustive mapping of the five outcome variants. The renderer checks membership
before conditions in the same request-then-response order. Empty, duplicate and
reordered subscription lists preserve their previous Boolean membership semantics;
the source configuration and schema validation are unchanged. A new outcome must
extend the exhaustive mapping as well as the existing capability/producer guards.

## Alternatives and limits

Capabilities describe statically supported producer paths, not arbitrary script,
condition, or dynamic routing reachability. DSL Ignore retains hard-error
FailClose even without a request-body builtin because regex scanners can surface
BodyAccess. OIDC nonce callbacks use JWKS independently of bearer verification
mode. Session renewal also reaches JWKS independently of that mode: an expired
introspection result followed by a refresh singleflight timeout calls
`verify_jwt_token_allow_expired` before checking stale-token leeway. Capabilities
therefore include both outcomes when `!bearer_only && renew_access_token_on_expiry`,
even without nonce validation. Introspection cache TTL is bounded by token expiry;
the regression fixture injects expired claims as a verifier input, not as evidence
that the normal cache retains expired tokens. No authentication or cache behavior
was changed to accommodate dye.

Collision regression inputs must each pass standalone plugin and dye validation
before combining them. Default RegionRoute has no dye capability, so using it
as a collision fixture can mask unrelated errors. The documented RegionRoute
dye example includes a real route rule and is checked through `validate_plugins`
in both languages. Entry disable is inert; condition-skipped active entries keep
anti-spoof stripping.

Response aggregation and post-commit header rewriting were rejected. No new
public dye capability is exposed. At the D0083 implementation boundary, three
frozen local-only named-jump fixtures still subscribed to both reject and fallback
while their policies permitted only one. The user subsequently authorized fixing
the failing integration tests. The fixture alignment and its preserved security
assertions are recorded in
[denied named jumps fail closed](../../routing/regression-guards/denied-named-jump-fails-closed.md).
Unsupported subscriptions are not grandfathered. No logging or metrics are added.

The 2026-09-10 fixture follow-up is now verified: all four actual named-jump
plugin manifests pass resource validation and Gateway dye-plan compilation,
and `EdgionPlugins_DynamicInternalUpstream` passes in the default integration
sweep (`tmp/integration_testing/testing_20260910_154454/report.log`). The sweep
has 237 passing report entries, no failures or timeouts; Build alone is skipped
because canonical preparation ran separately. The fixture task is closed.

## Verification and re-evaluation

Focused tests cover effective-policy/stage matrices, Controller/Gateway error
parity, inert entries, generation replacement/recovery, Canary's retained strip
plan, and producer/declaration inventory. Executable producer and renderer tests
remain necessary: a source inventory alone cannot prove branch reachability.
Verified in this fix: `cargo test -p edgion-resources --lib dye` (26 tests),
`cargo test -p edgion-gateway --lib dye` (20 tests), and focused Gateway tests
for `record_outcome`, ExtProc response streaming/trailers and failure modes,
Wasm traps, DSL error policy, Canary, DynamicInternalUpstream,
DynamicExternalUpstream, DirectEndpoint, and RegionRoute all passed. The complete
`plugins::runtime::session_adapter` unit-test group also passed (114 tests),
covering the real adapter's header rendering and sanitization paths.
`cargo fmt --all`, `cargo check --workspace --all-targets`, and
`cargo clippy --workspace --all-targets` passed (Clippy retains existing repository
warnings). Agent-doc, SSA force, metrics inventory, and Pingora source-policy
guards passed. Independent review returned LGTM. Full integration was not run;
the named-jump fixture incompatibility remained tracked at that time.

Follow-up review found and corrected the session-refresh JWKS capability omission,
invalid RegionRoute documentation examples, non-isolated collision test inputs,
and a mistaken K8s mirror requirement in the fixture task. Targeted re-verification
passed: resources `oidc` (7), resources `dye` (27), Gateway `dye` (21, including
the four refresh-timeout producer cases), Gateway `record_outcome` (62), the
existing cache-miss singleflight regression (1), resources `set_clobber` (4),
and `region_route_dye_docs` (1 test validating both languages). The refresh test
uses paused time and existing egress denial before dialing; no live IdP is needed.
The final follow-up diff passed independent review, workspace all-target check
and Clippy (existing warnings), formatting, and the four repository guards listed
above. Pre-existing unrelated workspace changes were preserved.

Re-evaluate on a new plugin producer, a changed failure policy or callback owner,
new stage applicability, a header commit boundary change, or changes to DSL
hard-error / OIDC nonce or session-refresh paths. Any new public body/trailer dye mechanism needs an
explicit design and separate feature-owned integration coverage.

## Resolution references

- D0083; `define-stage-aware-dye-capability-and-producer-parity.md` — real defect,
  full repair authorized and verified; the selected task and duplicate deferred
  finding were removed after independent review and required checks.
