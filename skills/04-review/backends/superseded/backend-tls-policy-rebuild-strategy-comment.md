---
name: backend-tls-policy-rebuild-strategy-comment
description: Historical backends-F015 comment correction for a BackendTLSPolicy rebuild heuristic that no longer exists.
knowledge_type: superseded
---

# BackendTLSPolicyStore rebuild-strategy complexity comment

## Inactive conclusion

This page is historical. `backends-F015` corrected the complexity comment for a
former strategy that cloned the complete reverse index, rescanned policies for
affected targets, and switched to a full rebuild according to a small-change
ratio heuristic. That implementation and its threshold no longer exist.

Current BackendTLSPolicy publication is governed by
[the incremental and atomic publication guard](../regression-guards/backend-tls-policy-incremental-atomic-publication.md).
Do not use the former cost model or threshold as evidence about current code.

## Historical evidence

The old comment correctly noted that cloning the then-current index cost
O(P*R), affected-target rescans cost approximately O(A*P), and a full rebuild
cost O(P*R), where P was total policies, R average target references, and A
affected targets. The former `A < P/10` choice was a small-change heuristic, not
an algebraic proxy for `A < R`.

This records the original comment-only result without claiming that its removed
implementation remains verified.

## Re-evaluation

Revisit only if the removed heuristic or an equivalent global-scan strategy is
reintroduced. Reviews of current target-local publication should use the
successor guard above.

## Resolution references

- `backends-F015` — historical comment-only fix, 2026-05-24.
- `incremental-publication-02-backend-tls-policy` — successor implementation,
  2026-09-08.
