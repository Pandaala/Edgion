# Deferred Findings Index

These entries are backlinks only. Each linked `tasks/todo` file remains the single editable source of truth for its issue.

| Finding | Status | Canonical task |
|---------|--------|----------------|
| D0001 — Define complete ExternalName DNS cache identity and lifecycle | needs-discussion | [campaign finding](D0001-external-name-dns-cache-identity-lifecycle.md) |
| Precompile backend port resolution and fail closed on missing mappings | pending | [task](../../../todo/backend-port-resolution-precompiled-fail-closed.md) |
| Fail closed on invalid Gateway BackendTLSPolicy candidates | pending | [task](../../../todo/backend-tls-policy-defensive-validation-fail-closed.md) |
| Make BackendTLSPolicy publication truly incremental | pending | [task](../../../todo/backend-tls-policy-true-incremental-publication.md) |
| Promote BackendTLSPolicy handling atomically to v1 only | pending | [task](../../../todo/backend-tls-policy-v1-only-atomic-promotion.md) |
| Publish Service backend traffic policy through one immutable snapshot | pending | [task](../../../todo/backend-traffic-policy-atomic-service-snapshot.md) |
| Conf-Sync certificate watch content signature | pending | [task](../../../todo/conf-sync-cert-watch-content-signature.md) |
| Conf-Sync generic watch malformed event recovery | pending | [task](../../../todo/conf-sync-generic-watch-malformed-event-recovery.md) |
| Conf-Sync rotation transactional address selection | pending | [task](../../../todo/conf-sync-rotation-transactional-address-selection.md) |
| Fail closed on denied L4 backend references | pending | [task](../../../todo/l4-backend-ref-denied-fail-closed.md) |
| Keep the load-balancer panic-mode gauge current across lifecycle exits | pending | [task](../../../todo/lb-panic-mode-metric-lifecycle.md) |
| Capture Service runtime health state once per load-balancer selection | pending | [task](../../../todo/lb-selection-service-runtime-state-capture.md) |
| Run mTLS whitelist verification once for the leaf certificate | pending | [task](../../../todo/mtls-leaf-only-whitelist-verification.md) |
| Define typed mTLS allowed SAN semantics | pending | [task](../../../todo/mtls-typed-allowed-san-semantics.md) |
| Make retry-budget generations and window rotation coherent | pending | [task](../../../todo/retry-budget-generation-and-window-coherence.md) |
| SSE window strict residency and continuity | pending | [task](../../../todo/sse-window-strict-residency-and-continuity.md) |
| Retain TLS last-known-good only for the same resource identity | pending | [task](../../../todo/tls-last-known-good-resource-identity.md) |
| TLS `SslCtx` owned handoff and exactly-once audit | pending | [task](../../../todo/tls-ssl-ctx-owned-handoff-and-exactly-once-audit.md) |
| TLS store true incremental and atomic publication | pending | [task](../../../todo/tls-store-true-incremental-atomic-publication.md) |
| Bound upstream-authority template rendering before allocation | pending | [task](../../../todo/upstream-authority-bounded-template-rendering.md) |
