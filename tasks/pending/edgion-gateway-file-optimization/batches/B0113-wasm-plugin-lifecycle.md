# B0113 — Wasm Plugin Lifecycle

## Scope

- Historical D0075 deferral was resolved externally in `256733d7`; current evidence lives in `skills/04-review/wasm/intentional-design/bounded-oci-artifact-memory-cache.md`. Original batch decisions and test results below remain historical; no fresh Wasm execution is claimed.
- Baseline: `fe73e84b`
- Pipeline state: commit-ready
- Active anchor crate: `edgion-gateway`
- Owned file: `edgion-gateway/src/plugins/http/wasm/plugin.rs`
- Baseline blob: `b512afe16622a10fa3429077bdc9523ef552310d`
- Implementation allow-list: `edgion-gateway/src/plugins/http/wasm/plugin.rs`, `edgion-gateway/src/plugins/http/wasm/host_abi.rs`, `docs/en/user-guide/http-route/filters/edgion-plugins/wasm.md`, and `docs/zh-CN/user-guide/http-route/filters/edgion-plugins/wasm.md`
- Forbidden files: every file outside a reviewer-approved exact allow-list, every existing `edgion-tests/**` path, and campaign-protected user paths

## Analysis boundary
- Read the file completely and trace plugin compilation/publication, VM generation and callback lifecycle, request/response hook ordering, local responses, body and streaming boundaries, callout/shared-data ownership, failure policy, panic isolation, diagnostics, hot reload, and deterministic tests.
- Preserve canonical request `BodyStorage`, streaming upstream responses, canonical access-log ownership, redaction, binary-owned logging, observability restraint, cross-cutting reuse, egress/cache ownership, no tracing, and the current fixed Wasm failure policy.
- Existing integration tests remain frozen. Prefer deterministic crate-local coverage; any genuinely necessary new integration coverage becomes a separate issue instead of an edit to existing integration files.

## Handoff
- Analyst: independent current-HEAD read-only analysis completed after tracing the full plugin/VM/host-ABI/session/body/callout/shared-data/test chain and the B0107–B0112 boundaries.
- Decision: change, medium severity and high performance value. The response-body hook currently copies every `Bytes` chunk into a `Vec`, the host ABI clones that complete buffer again for every guest read and then allocates the selected range, and the pooled VM retains the largest chunk capacity after `clear`.
- Complexity: moderate; no plan-first gate. The representation is private, the response remains current-chunk-only and read-only, and the public ABI and hook order do not change.
- Implementation: use clone-cheap `Bytes` ownership for the current response chunk, borrow the selected range directly into the guest-memory write, and drop the response reference before the lease returns to the pool. Preserve the existing owned-buffer behavior where snapshots require ownership.
- Documentation: correct stale source comments that describe `body:` as the request-body capture gate; exporting the request-body hook selects lazy `BodyStorage` capture and `body:` adds only pre-admission policy. Correct English and Chinese user documentation that still claims response-body Pause emits a per-module warning; the hot path records only its bounded metric.
- Preserve: all Proxy-Wasm status/range/empty semantics, one unavoidable guest-memory copy, whole-request-body capture, request mutation publication, current-chunk response streaming, Pause/trap behavior, fixed failure policy, generation/hot-reload/callout/shared-data/local-response/access-log ownership, and no request-path tracing.
- Tests: add deterministic inline pointer/backing-storage, range, and release regressions; run the complete Gateway Wasm unit set. Existing integration coverage remains frozen and sufficient, so no integration issue is needed.
- Freshness: external HEAD advanced from `fe73e84b` to `26461c83` through routing integration tests and review knowledge only. The four allow-listed blobs remain `b512afe1`, `583845ae`, `38bcfd64`, and `8ac94960`, with no worktree diff.
- Deferred: header-mutation transactionality remains owned by D0068; D0073, D0074, and D0075 remain separate and are not duplicated.
- Implementation: changed the current response-body storage to clone-cheap `Bytes`, retained owned snapshots for the other ABI buffers, wrote borrowed ranges directly to guest memory, and cleared the response reference before lease return. Added backing-pointer, range, and hook-to-pool uniqueness regressions; corrected the stale source and bilingual user documentation.
- Implementation scope: exactly the four approved files. No integration test, public ABI, schema, cache, metric, logging, failure-policy, or async-order change was made.
- Final review: initially Must Fix because two nearby source comments still described `body:` as the capture gate and implied an unsupported production binary. The same implementer corrected them to the existing prebuffer-or-lazy-capture contract, with `Unimplemented` limited to mock or alternate adapters; independent re-review then approved the batch with no remaining findings.
- Final freshness: external HEAD advanced again to `1408db41`, changing root rules, Cargo manifests, and unrelated Controller/backend/context/docs/knowledge files. The implementer reread the current root rules; all four allow-listed baselines were unchanged before implementation and no external change was included or reverted.
- Verification: all 194 focused Gateway Wasm tests passed. `cargo fmt --all`, locked workspace all-target check and Clippy, locked Gateway binary build, SSA, metrics inventory, Pingora source, Gateway no-tracing, English-only, diff, and refreshed frozen-path guards passed on `1408db41`; Clippy emitted only the existing warning set. Agent-doc validation's sole failure is an external unchanged review page linking to the absent `tasks/todo/webhook-sliding-window-epoch-coherence.md`; B0113 does not touch that page or its target.
- Commit: `05875438`
