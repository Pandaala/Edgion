# B0309 — Plugin Runtime Native CORS Refresh

## Scope

- Baseline: `fea8d812`.
- Pipeline state: terminal candidate; examined-no-change after fresh independent review at `fea8d812`.
- Active anchor crate: `edgion-gateway`.
- Target: `edgion-gateway/src/plugins/runtime/gateway_api_filters/cors_filter.rs`.
- Exact technical write scope is not approved during analysis. Existing `edgion-tests/**`, campaign records, unrelated modules and user-protected paths are read-only.

## Context and value gate

- External changes after B0124 widened validation diagnostics and added credentialed `exposeHeaders` wire coverage. The target later changed again in `fcd34424`, so analysis must use current HEAD rather than the older range note.
- The analyst must inspect shared resource validation and Controller/Gateway parity, origin matching, preflight/simple-response header construction, credentialed wildcard semantics, Vary ownership, inline bounded headers, phase dispatch, failure behavior, access-log/observability and deterministic tests before deciding change, examined-no-change or defer.
- Revalidate D0085/D0086 and maintained native-CORS knowledge. Existing integration tests remain frozen.

## Analysis and review

- Analyst `b0309_native_cors_analysis` traced shared resource validation, Controller status/delegation, Gateway publication, both runtime phases, current v1.5 sources and frozen scenarios. Decision: examined-no-change.
- Reviewer `b0309_native_cors_review`: LGTM with no Must Fix or new deferred finding. Current origin/preflight/actual-response behavior, credentialed reflection/exposure rejection, append-preserving `Vary`, stable six-entry non-spilling header storage and PluginLog-only observability remain coherent.
- Full v1.5 validation/default/publication parity remains wholly owned by D0085; fragmenting it into this single-file refresh would preserve the existing asymmetry. D0086 was resolved externally in `fcd34424`; only its two stale INDEX rows require coordinator reconciliation.

## Verification

- Native CORS tests passed 14/14, D0086 resource validation 1/1, Controller status/delegation 2/2 and Gateway publication 1/1.
- Locked Gateway all-target check, formatting, Gateway no-tracing, diff and frozen-path checks passed during independent review. No integration test was run or modified.
- Applied format plus locked workspace all-target check and Clippy passed. Agent-doc, SSA, metrics, Pingora source, Gateway no-tracing, English-only, diff and frozen-path guards all passed; Clippy retained only existing warnings.

## Handoff

- Ledger/changelog updated: yes.
- D0085 retained; D0086 INDEX reconciled without recreating its retired task.
- Commit: pending automatic local commit.
