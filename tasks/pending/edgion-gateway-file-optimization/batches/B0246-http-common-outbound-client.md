# B0246 — HTTP Common Outbound Client

## Scope

- Pipeline state: analysis-ready; refresh required after B0245 changes its supporting client documentation.
- Active anchor: `edgion-gateway`.
- Baseline: `8fc76e83`.
- Target: `edgion-gateway/src/plugins/http/common/outbound/client.rs`.
- Target blob: `b26c50682f43165dcb74af512c56e4661335ea48`.
- Inspect the complete outbound client wrapper, canonical construction and transport ownership, consumers, tests and relevant review precedent.
- Root owns Cargo and all writes. Existing integrations, external todos and handoff/WAF paths remain protected.

## Analysis Outcome

- Decision: change, complex/plan-first. Active health calls `pick_url` directly and bypasses normal `dial_url` literal/Service address checks; its raw base-plus-path concatenation permits authority replacement by an unvalidated configured path.
- `AnchorPinnedResolver` can return a different unsafe EndpointSlice address than the selected address already checked by `dial_url`, violating mandatory validation at every final connection path. `EndpointPinnedResolver` provides the existing local precedent.
- Exact proposed code allow-list: `outbound/client.rs`, `outbound/pinned_resolver.rs`, and the necessary cross-crate admission support in `edgion-resources/src/resources/common/external_http.rs`. Validate a safe root-relative health path, construct and defensively validate the final probe URL, filter every resolver result through the canonical backend-address validator, and remove the unused health-task label clone.
- Pure admission, URL and resolver unit coverage is sufficient; no integration edit or deferred integration issue is needed. Revalidate all target/support blobs after B0245 before promotion.
