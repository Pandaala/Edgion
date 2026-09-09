# B0207 — TLS Termination Proxy

## Scope

- Pipeline state: analysis-ready; queued, not the active mutation batch.
- Active anchor crate: `edgion-gateway`.
- Target: `edgion-gateway/src/routes/tls/proxy.rs`.
- Baseline: `c70aa9fe`.
- Analyst: `b0207_tls_proxy_analysis`.
- Investigate connection ownership, route selection, backend setup, stream plugins,
  transport lifetime and canonical observability through actual call chains.
- D0112 and D0114–D0117 remain separate until evidence establishes a safe scope.
- No implementation authorized; existing integrations and user-owned paths are protected.

## Analysis Outcome

- Decision: change; simple. Target blob `ba2521457d4d26ab1da91568f2f405b535375719` at `c70aa9fe`; no tests run by analyst.
- Restrict route-table guard to a synchronous selection helper, retaining only the owned selected unit across plugin/connect/relay awaits. Add Weak production-helper lifetime, generation and miss coverage.
- Bind and borrow the owned Pingora SSL digest within a synchronous block; copy only the four metadata fields actually installed in context instead of cloning the full TlsConnMeta and unused client-certificate/log/error data. Do not retain the digest across connection awaits.
- Borrow backend endpoint address with its existing backend Arc lifetime; keep bracket-safe conversion and all safety/PP2 behavior. Correct local per-port ownership comments to per-listener.
- Exact proposed scope: target, new `skills/04-review/routing/regression-guards/tls-terminated-connection-ownership.md`, and routing index. No shared helper abstraction or temporary SNI take/restore.
- Preserve plugin and post-select fail-close, weights, completion/outlier tokens, retry/timeout, transport/backpressure and all output fields. Verify metadata Some/None, mTLS/matched/version identity if a production installer helper is extracted; run TLS and existing context/log tests.
- Expanded D0116 with TlsConnMeta/SSL-handshake SNI output evidence; no masking fix in this batch. D0112/D0114/D0115/D0117 remain separate.
