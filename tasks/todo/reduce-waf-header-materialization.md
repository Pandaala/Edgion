# Reduce WAF Header Materialization

## Meta

| Key | Value |
|-----|-------|
| Created | 2026-09-02 |
| Status | pending |
| Type | performance |
| Priority | P2 |
| Issue | N/A |

## AI guide summary

Eliminate the WAF path's per-header temporary vectors and duplicate transaction copy without weakening object safety, lifetimes, validation, or request/response body ownership.

## Requirements

- Benchmark the current request and response header ingestion path with representative and header-heavy workloads.
- Design either a borrow-scoped object-safe raw-header visitor or a transaction API that safely consumes owned header storage.
- Cover `PluginSession`, the Pingora adapter, WAF plugin input, `edgion-waf` transaction storage, and test mocks as one coherent boundary.
- Preserve ordered duplicate headers, original bytes, transaction lifetime safety, bounded memory, canonical access-log ownership, and no-tracing.
- Do not modify existing integration-test files; add a separate integration issue only if observable lifecycle behavior changes.

## Acceptance criteria

- Request and response WAF header ingestion no longer allocates a temporary vector for every name and value before transaction storage.
- Benchmarks demonstrate the allocation and throughput impact without moving unbounded ownership into the transaction.
- All relevant Gateway, resources, `edgion-waf`, format, compile, Clippy, observability, and frozen-path guards pass.
