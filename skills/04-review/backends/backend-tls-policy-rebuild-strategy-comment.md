---
name: backend-tls-policy-rebuild-strategy-comment
description: Use when reviewing BackendTLSPolicyStore::update rebuild-strategy complexity-comment findings — the comment now states real costs; the issue's A < P/10 proxy wording is rejected.
---

# BackendTLSPolicyStore::update Rebuild-Strategy Complexity Comment

**Status / verdict**: fixed (backends-F015, 2026-05-24)

When reviewing findings of the form "`BackendTLSPolicyStore::update`'s rebuild-strategy comment claims incremental is `O(n*m)` and full rebuild is `O(m)` / the complexity comment drops the avg-target-refs (R) factor / a reader cannot reason about the incremental-vs-full-rebuild crossover from the comment (backends-F015)": **fixed (backends-F015, 2026-05-24)**. The comment in `update` (`edgion-gateway/src/backends/policy/backend_tls/backend_tls_policy_store.rs`) now states the real costs — incremental clones the current index (`O(P*R)`) then rescans all policies for each affected target (`~O(A*P)` match checks); full rebuild is a single `O(P*R)` pass (A=affected_count, P=total_policies, R=avg target_refs per policy). The `let use_incremental = affected_count < total_policies.max(10) / 10` logic is unchanged (comment-only, no behavior change). The issue's suggested replacement (`incremental is O(A * P)` + "A < P/10 as a proxy for A < R") is **not accepted as written**: it omits the index-clone cost and the inner R factor, and `A < P/10` is not a proxy for the crossover `A < R` (R and P are independent) — the threshold is a "small change ratio" heuristic, not the exact crossover. Re-reports of the wrong-complexity-comment form are not accepted; re-reports proposing the issue's `A < P/10 ≈ A < R` wording are also not accepted. Verify the current comment before re-reporting.
