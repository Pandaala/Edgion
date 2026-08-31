# Deferred Findings Index

These entries are backlinks only. Each linked `tasks/todo` file remains the single editable source of truth for its issue.

| Finding | Status | Canonical task |
|---------|--------|----------------|
| D0001 — Define complete ExternalName DNS cache identity and lifecycle | needs-discussion | [campaign finding](D0001-external-name-dns-cache-identity-lifecycle.md) |
| D0002 — Make EndpointSlice ownership transitions generation-safe | needs-discussion | [campaign finding](D0002-endpoint-slice-ownership-transitions.md) |
| D0003 — Replace Gateway backend TLS identities authoritatively | needs-discussion | [campaign finding](D0003-gateway-backend-tls-authoritative-replacement.md) |
| D0004 — Make health task/status generations cancellation-safe | needs-discussion | [campaign finding](D0004-health-task-status-generation-ownership.md) |
| D0005 — Define the active health-check TLS contract | needs-discussion | [campaign finding](D0005-active-health-check-tls-contract.md) |
| D0006 — Bind outlier state and gauge updates to live generations | needs-discussion | [campaign finding](D0006-outlier-generation-gauge-lifecycle.md) |
| D0007 — Define Elasticsearch bulk item-level retry semantics | needs-investigation | [campaign finding](D0007-elasticsearch-bulk-item-retry-semantics.md) |
| D0008 — Isolate Kafka producer tasks by runtime generation | needs-investigation | [campaign finding](D0008-kafka-producer-generation-shutdown.md) |
| D0009 — Bound aggregate Kafka topic-channel memory | needs-discussion | [campaign finding](D0009-kafka-aggregate-channel-memory-bound.md) |
| D0010 — Enforce Kafka broker egress and DNS policy | needs-investigation | [campaign finding](D0010-kafka-broker-egress-dns-policy.md) |
| D0011 — Define a DataSender graceful shutdown contract | needs-discussion | [campaign finding](D0011-data-sender-graceful-shutdown-contract.md) |
| D0012 — Refresh OTLP masking and health integration coverage | needs-implementation | [campaign finding](D0012-otlp-integration-masking-health-refresh.md) |
| D0013 — Define a strict Redis endpoint contract | needs-discussion | [campaign finding](D0013-redis-endpoint-contract.md) |
| D0014 — Reconcile the Redis configuration contract | needs-discussion | [campaign finding](D0014-redis-configuration-contract.md) |
| D0015 — Enforce Redis DNS and egress policy | needs-investigation | [campaign finding](D0015-redis-dns-egress-policy.md) |
| D0016 — Define Redis pool health and safe connection logging | needs-discussion | [campaign finding](D0016-redis-pool-health-logging.md) |
| D0017 — Add successful Redis TLS integration coverage | needs-implementation | [campaign finding](D0017-redis-tls-integration-coverage.md) |
| D0018 — Make disabled outbound TLS inert before publication | needs-investigation | [campaign finding](D0018-disabled-tls-publication-readiness.md) |
| D0019 — Make Webhook sliding windows time-coherent | needs-discussion | [campaign finding](D0019-webhook-sliding-window-time-coherence.md) |
| D0020 — Refresh the shared LinkSys lifecycle documentation | needs-implementation | [campaign finding](D0020-link-sys-lifecycle-webhook-doc.md) |
| D0021 — Make partial LinkSys provider replacement authoritative | needs-discussion | [campaign finding](D0021-link-sys-cross-provider-partial-replacement.md) |
| D0022 — Refresh the LinkSys documentation set | needs-implementation | [campaign finding](D0022-refresh-link-sys-documentation-set.md) |
| D0023 — Define OTLP egress enforcement and safe endpoint identity | needs-investigation | [campaign finding](D0023-otlp-egress-safe-endpoint-identity.md) |
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
