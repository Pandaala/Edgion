# B0246 — HTTP Common Outbound Client

## Scope

- Pipeline state: changed; implementation, independent reviews and verification complete, awaiting local commit.
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

## Refresh Baseline

- Current HEAD: `ceb13835`.
- `client.rs`: `7fa15c2cf12c1249382192044727fdb2eed1b0de` after B0245's passive-recovery documentation correction.
- `pinned_resolver.rs`: unchanged at `16b37cdd622b7c3bf3894c2d080b3cc4d0909d21`.
- `external_http.rs`: unchanged at `ea0e8d7d8361c5f8025614ff81cbc02e6f2406f5`.

## Refreshed Analysis Outcome

- B0245 changed only passive-recovery documentation in `client.rs`; its executable changes stay in `call.rs`, which remains outside B0246. All three original security findings remain current.
- Validate the active health path as a safe root-relative reference at shared admission. Runtime composition must use parsed URL operations, preserve the original origin and query semantics, and defensively validate the final literal destination before sending; a leading slash alone is insufficient because scheme-relative/backslash forms can replace authority.
- Preserve URL-target unspecified/`blockPrivate` policy, integration-testing loopback behavior, DNS `EgressResolver`, and RFC1918 Service endpoints. Every `AnchorPinnedResolver` address must pass the canonical backend-address validator before rotation; an all-rejected pool yields no candidate.
- Combined passive+active health has a separate confirmed ownership defect: passive alone opens the circuit, active may recover it, and active failures may open only in active-only mode. Active success must not reset B0245 passive counters/backoff.
- Exact frozen code allow-list: `edgion-gateway/src/plugins/http/common/outbound/client.rs`, `edgion-gateway/src/plugins/http/common/outbound/pinned_resolver.rs`, and necessary admission support `edgion-resources/src/resources/common/external_http.rs`. Remove the unused health-task label clone; add only pure/local unit coverage. No integration edit or deferred integration issue is needed; D0005 remains separate.

## Outcome

- Shared admission now accepts only safe root-relative active-health references, rejecting absolute, scheme-relative, raw-backslash, authority-replacing and fragment-bearing forms while preserving parsed path/query data and encoded path bytes.
- Active probes compose URLs through parsed components, recheck unchanged origin/no credentials and apply the same final literal policy as normal calls. URL targets retain unspecified/`blockPrivate` enforcement and DNS `EgressResolver`; Service targets keep RFC1918 support while canonical validation rejects unsafe loopback/link-local destinations.
- `AnchorPinnedResolver` filters every EndpointSlice candidate through canonical final-dial validation before cursor rotation; mixed pools expose only safe addresses and all-rejected pools return no candidate without system-DNS fallback.
- Active-only failures open at threshold. When passive health is configured, active failure never opens its circuit; active success may recover either mode without modifying passive failures, half-open baseline or backoff. Removed the unused task label clone.
- Independent plan reviewer `b0246_plan_review`: APPROVED. Independent final reviewer `b0246_final_review`: LGTM with no findings after verifying URL parser, IPv4-mapped address, resolver rotation and active/passive state boundaries.
- Verification: outbound client 30/30, pinned resolver 4/4, shared external HTTP 22/22, ForwardAuth 6/6, AI Guard 49/49, Guardrail 20/20 and Webhook 46/46. Workspace format/check/Clippy (existing warnings), agent docs, SSA, metrics inventory, Pingora source, Gateway no-tracing, English-only, diff and 3,136-file freeze passed.
- No integration edit or new issue. Existing D0005 remains independent.
- Commit: pending; never push.
