# B0040 — Gateway API ExtensionRef

## Scope

- Original baseline: `7b0ffc010ad67ef78482b79f6fb7f1f06dbd6860`.
- Revalidation baseline after concurrent commits: `b8a87ae92810c876ad36f3472126bf925618a198`.
- Pipeline state: `complete`.
- Active anchor crate: `edgion-gateway`.
- Anchor file: `edgion-gateway/src/plugins/runtime/gateway_api_filters/extension_ref.rs`.
- Approved support files: `edgion-gateway/src/plugins/runtime/plugin_runtime.rs`, `skills/01-architecture/02-gateway/12-edgion-plugin-dev.md`, and `skills/04-review/conf-sync/intentional-design/cross-kind-config-not-atomic-extensionref-torn-state.md`.
- Existing `edgion-tests/**` files remained frozen. A concurrent external commit changed one frozen path; after exact provenance, content-hash, and mode verification, only that manifest entry was refreshed.
- Exact commit allow-list: the three approved support files, `integration-freeze.tsv`, and B0039/B0040 campaign ledger, batch, main, and changelog records.

## Context and value gate

- Decision: the anchor runtime is `examined-no-change`; its fail-closed execution, positional rule lookup, delayed backend lookup, and hit/miss pinning are correct.
- Generation evidence: each request captures one immutable `GatewayScope`; its `.scope` is the class-scoped `PluginScope`. Rule references resolve positionally from it, while backend references resolve lazily from the same scope and pin their first hit or miss across retries.
- Ownership evidence: `ExtensionRefFilter` consumes request projections and never queries the live `PluginStore`. A mismatched Gateway owner is rejected instead of mixing policy generations.
- Stage evidence: a declaration fans out to Request, synchronous UpstreamResponseFilter, per-chunk UpstreamResponseBodyFilter, and asynchronous UpstreamResponse stages.
- Knowledge drift: runtime comments and two canonical guidance pages still described direct live-store lookup, three stages, and obsolete cache ownership. Those descriptions were corrected without behavior changes.
- Freshness evidence: the complete mechanism and approved edits were revalidated after four concurrent commits advanced HEAD; none altered the ExtensionRef/publication behavior or the approved support files.

## Implementation

- Updated constructor comments to describe request-captured class-scope resolution and backend hit/miss pinning.
- Corrected the plugin development guide's four-stage fan-out and immutable request-scope ownership.
- Corrected the accepted cross-kind torn-state guidance to current `PluginPublisher`/`GatewayScope` publication and self-healing semantics.
- Refreshed the frozen manifest entry for `edgion-tests/integration/scripts/integration/link_sys/otlp_test_assertions.py` after verifying concurrent commit `736b77df274d1a8c7e5ff46ea02b8af138ed5c2c`, SHA-256 `2620c25f4bcab37cb45f718b6ae154e51d2356f05456b259024dac906e66caac`, and unchanged mode bits.

## Independent review

- Initial review found that the first documentation correction collapsed the `GatewayScope` wrapper into `PluginScope` and could mislead future maintainers.
- The implementer corrected the wording to retain `gateway_key` and `http_keys` ownership on `GatewayScope` and name `.scope` as the class-scoped `PluginScope`.
- Final independent correction review approved the anchor verdict, all three support edits, the exact concurrent-HEAD revalidation, and the one-entry frozen-manifest reconciliation with no remaining findings.

## Verification

| Check group | Result | Important note only |
|-------------|--------|---------------------|
| Focused tests | passed | 14 focused ExtensionRef/publication/runtime tests passed on the corrected implementation. |
| Full repository gates | passed | Format, workspace check/clippy, agent docs, SSA, metrics inventory, Pingora source policy, no-tracing, cargo-deny, and English-only checks passed; only the campaign's known lint and duplicate-dependency warnings remain. |
| Concurrent change audit | passed | Four external commits were inspected; none changed the B0040 mechanism or approved support files. |
| Final frozen-path verification | passed | All 3,442 recorded integration paths match after the single proven concurrent OTLP assertion update. |

## Handoff

- Ledger/changelog updated: yes.
- Commit: `3782366f41a793862d75c117fd4b06e2729fb207`.
