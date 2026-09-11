# Close Phase 4 Validation and Stage 8

## Meta

| Key | Value |
|-----|-------|
| Status | pending |
| Priority | P1 |
| Depends on | `09-phase4-rawwindow-integration.md` |

## Goal

Validate the complete streaming-first request/response WAF design and close
Stage 8 without broadening compatibility claims beyond executable evidence.

## Work

- Add tracked response-stream correctness and resource regression cases.
- Exercise ordinary downloads, compressed/encoded compatibility boundaries,
  SSE, empty responses, trailers, cancellation, and upstream failures.
- Verify Phase 1-5 exact-once lifecycle under allow, detection-only, request
  intervention, response intervention, abort, and hot reload.
- Run static and dynamic Controller/Gateway integration scenarios.
- Review all WAF metrics for fixed label domains and balanced gauges.
- Re-run the internal ARM64 benchmark and compare with the recorded baseline.
- Update the compatibility matrix, known-divergence ledger, user docs, skills,
  Stage 8 task status, and validation evidence.
- Obtain an independent code review before declaring closure.

## Required verification

- `cargo fmt --all`
- `cargo check --workspace --all-targets`
- `cargo clippy --workspace --all-targets`
- Targeted WAF, Gateway, Controller, resource, and integration tests
- `python3 cicd/checks/validate_agent_docs.py`
- `python3 cicd/checks/check_ssa_force.py`
- `python3 cicd/checks/check_metrics_inventory.py`
- `python3 cicd/checks/check_pingora_source_policy.py`
- `git diff --check`
- Run `cargo test --all` only with explicit user authorization.

## Exit gate

Stage 8 is complete only when raw, JSON, XML, and supported response rules use
bounded streaming paths; compatibility-only request snapshots remain explicit
and independently bounded; full response aggregation remains impossible; all
documented correctness, resource, integration, and ARM64 performance gates
pass.
