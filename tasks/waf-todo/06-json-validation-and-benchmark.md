# Close JSON Streaming Validation and Performance Gates

## Meta

| Key | Value |
|-----|-------|
| Status | pending |
| Priority | P1 |
| Depends on | `05-gateway-hybrid-request-body-path.md` |

## Goal

Produce sufficient correctness, resource-bound, integration, and ARM64
performance evidence to enable JSON streaming by default for the classified
rule subset.

## Work

- Add deterministic differential corpora without external production
  dependencies.
- Add fixed-seed randomized chunking and malformed-input cases.
- Extend the tracked internal benchmark with representative JSON request
  bodies, early/middle/late matches, zero-hit input, and adversarial tokens.
- Report retained request bytes, reusable scratch, parser work, p50/p95/p99,
  and throughput without portable timing assertions.
- Add static and dynamic Gateway integration scenarios for JSON allow, deny,
  invalid update retention, recovery, deletion, and recreation.
- Update metrics inventory, architecture knowledge, and user documentation.

## Required verification

- `cargo test -p edgion-waf --lib`
- Targeted Gateway WAF and request-body tests
- Targeted Controller/resource schema tests if configuration changes
- ARM64 release benchmark
- Workspace format, check, clippy, documentation, metrics, SSA, and Pingora
  source-policy guards

## Exit gate

JSON streaming has zero unexplained differential mismatch, bounded retained
memory across large bodies, passing hot-reload integration, and recorded ARM64
performance evidence.
