# B0214 — UDP Route Unit

## Scope

- Pipeline state: verified examined-no-change with owning documentation correction; awaiting local commit.
- Active anchor crate: `edgion-gateway`.
- Target: `edgion-gateway/src/routes/udp/route_unit.rs`.
- Baseline: `86e2002c`.
- Analyst: `b0206_tls_passthrough_analysis`, acting only as the B0214 analyst.
- Investigate immutable schema/selector ownership, plugin and annotation policy,
  and actual UDP session consumers; revalidate after B0211 handler changes.
- Implementation is limited to the single knowledge page below; existing integrations and user-owned paths remain protected.

## Analysis Outcome

- Decision: target examined-no-change with simple owning-knowledge correction. Target blob `8e7289a8ccc840eaa43c9013597a096a03b9fb32` at `86e2002c`; analyst did not review the concurrently active B0211 diff or run tests.
- Unit retains schema Arc and completes shared selector/key compilation before publication; first-rule, empty-backend, mixed-weight and denied/unsupported post-select policies match actual UDP consumers. Outer selector Arc removal offers insufficient cold-path value.
- Exact proposed technical scope is one original knowledge page: `skills/04-review/routing/regression-guards/udproute-stream-plugins-annotation-wiring.md`. Correct stale rule/handler ownership to UdpRouteUnit, nonexistent legacy runtime fields, old F-016 link and re-report checkpoints.
- Critically, 60 seconds is an inactivity threshold, not a maximum lifetime or guaranteed hot-reload-to-deny delay. Active sessions can retain their established decision; describe new-session dynamic store lookup without adding immediate invalidation requirements.
- Preserve useful historical evidence and current code/integration behavior. Existing broad manager-doc task does not cover this incorrect timeout claim; no duplicate issue required.
- At promotion revalidate B0211/B0212 changes and independently review the single-page correction. Unit five-test filter and shared selector are available for fresh smoke verification.

## Promotion and Implementation

- Analyst revalidated at `2f51deff`: unit blob unchanged, B0211/B0212 preserve key ownership and established-session bypass. Successful sends refresh activity, so neither the inactivity threshold nor cleanup interval gives a fixed hot-reload deadline.
- Implementer: `b0214_implementation`; authorized only the original owning knowledge page. No source, policy or integration edits. Independent final review required.

## Implementation and Verification

- Single-page correction complete, blob `7795fd2053cf26de89f897b502f5d7c910343d3e`; anchor unchanged. Current unit ownership, new-session enforcement and inactivity semantics now separated from historical evidence; stale link and blanket re-report suppression corrected.
- Implementer docs (1,265 files) and diff checks passed. Coordinator unit 5/5, selector 22/22, UDP 34/34, workspace format/check/Clippy, SSA, metrics, Pingora source, Gateway tracing, English-only and 3,120-file freeze passed.
- Independent reviewer: `b0214_final_review`; LGTM, no Must Fix or Should Fix at `2f51deff`, both hashes unchanged. Personally checked schema/unit/handler, actual plugin and established-session paths, both activity refresh branches and periodic cleanup. Historical evidence and corrected link no longer imply a fixed revocation contract or suppress new reports. Independent docs (1,265 files) and diff passed; no runtime/integration changes. Commit pending; no push.
