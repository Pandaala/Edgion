# Open plugin types implementation plan

## Scope and authorization

The user authorized implementation of the open-type specification and then explicitly expanded scope to Stream plugins. This includes both connection and TLSRoute stages of EdgionStreamPlugins. No commit or push is requested.

## Serial work plan

1. Add schema-hidden, non-executable sentinels to the three existing enums. Share the config-discard deserializer. Preserve exhaustive matches and typed known configs.
2. Report indexed unsupported-type errors from Controller preprocessing, independent of enable or conditions. Preserve conf_sync serialization of invalidity.
3. Reuse HTTP invalid-runtime sealing. Stream sources containing an unsupported type deny every demanded connection/TLSRoute attachment, independent of policy bypass; direct per-stage runtime constructors also deny unknown entries.
4. Remove five CRD discriminator lists, retaining structural fields and nested condition enums. Replace the old whitelist guard with an open-string contract for six stages and supported-name exclusion tests.
5. Verify list decoding, known-config rejection, Controller status, schema validation, serialization, actual HTTP rejection, Stream denial, policy masks, and valid correction. Run workspace check/clippy and targeted tests, plus applicable repository guards. Do not run the entire workspace test suite.
6. Update design/review knowledge and development guides. Record actual scope, checks, and limitations before delivery.

## Status

Implementation complete. No commit or push was performed. Existing unrelated mesh work is untouched.

## Scope amendment

The original spec excluded Stream. That exclusion is superseded by the user's follow-up. HTTP and both Stream enums use the same unsupported-config deserializer. The Stream source gate must inspect both lists even when an attachment demands only one stage, so an unknown entry cannot turn into a successful empty chain.

## Verification record

| Check | Result |
| --- | --- |
| Workspace all-target check | Passed on final code |
| Workspace all-target Clippy | Passed without warnings after the TLSRoute payload adjustment |
| Format and diff whitespace | Passed |
| HTTP/Stream CRD and nested condition guards | 14 passed |
| Resources HTTP plugin validation | 45 passed |
| Controller unsupported-input/schema/status selection | 22 passed, including final-code rerun |
| Gateway unsupported-input selection | 31 passed |
| HTTP generation regression | 11 passed on final code |
| Publication and hot-reload regression | 22 passed on final code |
| Stream regression | Initially 87 passed, 5 failed, 1 ignored; follow-up fixture repair: 92 passed, 0 failed, 1 ignored |
| Agent docs, unit-test layout, SSA force, metrics inventory, Pingora source policy, Gateway API transform contract, Gateway tracing guard | Passed |

No full workspace test suite or live Kubernetes cluster test was run. Typed list decoding and
Controller schema/status behavior were exercised locally. The new rejection tests cover HTTP,
connection, and TLSRoute execution, disable/policy bypass, and HTTP full/delta correction.

### Stream test failures and follow-up repair

An isolated, unmodified worktree at `58848100` ran the same `plugins::stream` selection:
84 passed, the same 5 failed, and 1 ignored. The final change adds 3 passing Stream tests.
The failing test-name sets were compared for exact equality; the initial open-type change
did not modify their store/bridge fixtures. They failed on missing published scopes/views
before plugin execution.

- `connection_filter_bridge::tests::listener_annotation_can_be_added_and_removed_after_startup`
- `connection_filter_bridge::tests::stream_parent_ports_preserve_scope_across_gateway_publications`
- `stream_plugin_store::tests::full_set_publishes_then_empty_full_set_clears`
- `stream_plugin_store::tests::local_budget_survives_full_delta_delete_and_disabled_generations`
- `stream_plugin_store::tests::partial_delta_preserves_replaces_adds_and_removes_snapshots`

The user subsequently authorized investigating and repairing these failures. On 2026-09-26,
a fresh build reproduced all five. Three fixture construction sites omitted Controller-owned
`spec.currentStatus`: the shared store Gateway (three failures), the dynamic-annotation Gateway,
and the TCP/UDP Route fixtures. Missing listener status excludes a Gateway from publication;
missing accepted parent status excludes Route attachments. This is the intended production
contract, not a runtime defect.

Two test files now use the existing `test_current_status::{from_value, processed}` helpers.
All original assertions remain, including shared rate budgets, immutable snapshots, parent
port selection and annotation hot reload. No production behavior changed. Fresh verification:
Stream 92 passed, 0 failed, 1 existing ignored; publication 22 passed; listener index 17 passed.
The latter suites retain missing-current-status rejection coverage. Gateway library/test Clippy,
formatting, diff whitespace, unit-test layout and agent-document validation passed. No full
workspace suite or live integration run was performed.

## Implementation adjustment

Adding a unit sentinel exposed Clippy's large-variant warning on the formerly single-variant
TLSRoute enum. Its existing IpRestriction payload now uses Box, keeping the enum compact and
preserving serde/schema shape. This adds cold configuration allocation, not request-path work.
No full custom deserializer, mirrored enum, or watcher rewrite was needed.


## Final scope and operating contract

- Seventeen production/config files changed: fifteen Rust files and two committed CRDs.
- Deploy the updated CRDs once to remove the existing five whitelists. Later additions to the
  three Rust plugin enums do not require another CRD type-list edit or generator invocation.
- Known configs, nested condition enums, policy catalogs, and explicit factory classifications
  remain validated. The sentinel is not a supported plugin or a wildcard match for future variants.
- Unknown names/config content are not retained in typed views. Diagnostics use stage/index;
  Kubernetes retains the submitted spec. New common schema fields can still require a CRD update.
- The actual schema-validator tests admit an arbitrary future plugin name without a CRD change.
  No temporary synthetic production variant was committed, and no mixed-version or live-cluster
  compatibility claim is made.
