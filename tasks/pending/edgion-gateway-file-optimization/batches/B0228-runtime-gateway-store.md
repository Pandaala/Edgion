# B0228 — Runtime Gateway Store

## Scope

- Pipeline state: verified examined-no-change (owning docs corrected); awaiting local commit.
- Active anchor crate: `edgion-gateway`.
- Target: `edgion-gateway/src/runtime/store/gateway.rs`.
- Baseline: `a63a88ca`.
- Analyst: `b0228_gateway_store_analysis`.
- Inspect complete store and handler/publication/consumer ownership. Existing D0003/D0123 are distinct deferred publication issues, not permission for partial fixes.
- Revalidate the carried architecture claim of a removed `GatewayStore::get_gateway` in `10-runtime-store.md`. The merged-snapshot page's old Vec shape belongs to the adjacent listener store analysis unless evidence requires this target's support.
- No writes authorized. Existing integrations, external todos and handoff/WAF paths remain protected.

## Analysis Outcome

- Analyst recommends examined-no-change for source with a simple owning-document correction at `a63a88ca`; target blob `569e461814efcf8d0e22b8be99a98213c46caeb6`. No source/test writes or Cargo run.
- Full 73-line map, all production consumers and cache dispatch show low-frequency raw configuration mutation/snapshot under a write guard, followed by derived TLS/PGIS rebuild after release. Request routing reads derived views; admin diagnostics use count. CacheData serializes callbacks, so lock release alone is not evidence of concurrent handler lost updates.
- Cold double lookups and owned snapshots offer no demonstrated value for changing APIs/ownership. Existing D0003/D0123 remain pending and must not be partially repaired or claimed resolved.
- Exact proposed technical support: only `skills/01-architecture/02-gateway/10-runtime-store.md` GatewayStore section. Remove nonexistent get_gateway, add count diagnostic purpose, and replace the incorrect high-write-frequency rationale with raw-map/owned-snapshot/derived-reader ownership. Leave other stores, adjacent snapshot Vec shape and response sections untouched.
- Original knowledge: runtime/store/startup/ConfHandler architecture, concurrency/listener knowledge, design/coding/testing rules. Normal document/compile/guard verification; no new integration requirement. Simple documentation implementation then independent review.

## Promotion

- Mechanically refreshed at `1456c442`: only unrelated server-header rustdoc changed since analyst baseline `a63a88ca`; target, handler/cache/publication and owning architecture unchanged. Freeze passed.
- Exact technical allow-list: GatewayStore section of `skills/01-architecture/02-gateway/10-runtime-store.md` only. Target source remains no-change; D0003/D0123 stay separate.
- Coordinator scope: main task, ledger, changelog, B0227 SHA backfill, this record and subsequent queued analysis records. No protected external work or integrations may be modified/staged.

## Implementation

- Separate implementer `b0228_implementation` updated only GatewayStore API/ownership descriptions in the owning architecture page. Actual count/owned snapshot and low-frequency raw store boundary replace nonexistent lookup/high-write-frequency claims; no source/test/API change or cross-store atomicity claim.
- Fresh `b0228_final_review` checks full source/consumers and knowledge diff. Normal verification gates apply; compile/Clippy wait for an externally running `tls_passthrough_clienthello` test to release Cargo rather than competing for its build lock.

## Independent Review and Verification

- Final reviewer LGTM; no Must Fix/Should Fix. Full raw-store/handler/cache/admin/derived-reader evidence and scoped knowledge correction independently verified. D0003/D0123 remain pending, not repaired or dismissed.
- External Cargo work completed; fresh coordinator workspace check/Clippy passed (`/tmp/edgion-b0228-{check,clippy}.log`, existing warnings). Format, docs, SSA, metrics, Pingora source, Gateway tracing, English-only, diff and integration freeze passed. No new tests required for this documentation-only change; no historical test result presented as fresh.
- Ledger/changelog updated; source/integrations unchanged. Commit pending, local only.
