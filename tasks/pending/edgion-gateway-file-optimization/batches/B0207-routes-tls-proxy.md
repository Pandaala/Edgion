# B0207 — TLS Termination Proxy

## Scope

- Pipeline state: committed locally.
- Active anchor crate: `edgion-gateway`.
- Target: `edgion-gateway/src/routes/tls/proxy.rs`.
- Baseline: `c70aa9fe`.
- Analyst: `b0207_tls_proxy_analysis`.
- Investigate connection ownership, route selection, backend setup, stream plugins,
  transport lifetime and canonical observability through actual call chains.
- D0112 and D0114–D0117 remain separate until evidence establishes a safe scope.
- Implementation follows the exact authorized three-path scope below; existing integrations and user-owned paths are protected.

## Analysis Outcome

- Decision: change; simple. Target blob `ba2521457d4d26ab1da91568f2f405b535375719` at `c70aa9fe`; no tests run by analyst.
- Restrict route-table guard to a synchronous selection helper, retaining only the owned selected unit across plugin/connect/relay awaits. Add Weak production-helper lifetime, generation and miss coverage.
- Bind and borrow the owned Pingora SSL digest within a synchronous block; copy only the four metadata fields actually installed in context instead of cloning the full TlsConnMeta and unused client-certificate/log/error data. Do not retain the digest across connection awaits.
- Borrow backend endpoint address with its existing backend Arc lifetime; keep bracket-safe conversion and all safety/PP2 behavior. Correct local per-port ownership comments to per-listener.
- Exact proposed scope: target, new `skills/04-review/routing/regression-guards/tls-terminated-connection-ownership.md`, and routing index. No shared helper abstraction or temporary SNI take/restore.
- Preserve plugin and post-select fail-close, weights, completion/outlier tokens, retry/timeout, transport/backpressure and all output fields. Verify metadata Some/None, mTLS/matched/version identity if a production installer helper is extracted; run TLS and existing context/log tests.
- Expanded D0116 with TlsConnMeta/SSL-handshake SNI output evidence; no masking fix in this batch. D0112/D0114/D0115/D0117 remain separate.

## Promotion and Implementation

- Original analyst revalidated at `aba8fd24`: target blob and all production dependencies except the adjacent passthrough implementation remain unchanged. Personally reviewed B0206's helper and new knowledge; no cross-proxy helper extraction is needed.
- Weak coverage uses two distinct published hostname winners. Digest and extension ownership must end synchronously before connection awaits.
- Implementer: `b0207_implementation`; authorized the exact three paths and local tests above. Broader/complex deviations require independent plan approval first.
- Implementation was interrupted by an agent usage-limit error after partial target edits, before support documents or verified results. User requested continuation; the same implementer is resuming from the preserved worktree. No partial implementation has been staged or committed.

## Implementation and Verification

- Implementer completed the exact three technical paths after recovery. Target blob `c752fd026a25b7d225223eccd79192a98760ca3d`; no further source edits planned.
- Added inline selected-unit/snapshot lifetime and metadata field-preservation coverage. Initial test-only ClientCertInfo import used the wrong module and failed compilation; implementer corrected it to `crate::ctx::ClientCertInfo`. All final results must come from reruns after this correction.
- Independent reviewer: `b0207_final_review`; LGTM, no Must Fix or Should Fix at `aba8fd24`, target blob unchanged. Personally verified real digest producer/API/consumer, original stream ownership, four installed fields, unchanged transport/denial/logging and frozen integration boundaries. All five target Clippy warnings predate the batch; test-only import correction is also verified by successful Clippy test-target compilation. Independent format/diff/docs/freeze passed; reviewer test remains pending.
- Format/diff, documentation (1,264 files), SSA, metrics, Pingora, Gateway tracing, English-only, frozen paths, corrected workspace check, Clippy and Gateway build passed. Routes 597/597, TLS 54/54, logs 17/17, relevant request-scenario context 3/3 and independent proxy tests 2/2 passed.
- Independent wait diagnosis confirmed active test-binary compilation with advancing CPU time and sufficient disk, not a deadlock; the queue completed without process intervention.
- Broader `ctx::` execution and independent rerun reported 91 passed, three failed fixed-size layout assertions and one ignored. Independent diagnosis traced all three to committed HTTP layout changes (`90a45683`, `9b6b6fa0`, `8cbbed6c`), verified unchanged context/Cargo sources and no embedded dependency on this function-only TLS diff, and approved safe continuation with D0119. Separate grpc/context budget tests each pass; no claim of complete context or allocation-free verification is made. No assertion was edited to suppress failure.
- Commit: `ac3dfe7c`; no push.
