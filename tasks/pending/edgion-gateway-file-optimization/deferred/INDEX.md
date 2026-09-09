# Deferred Findings Index

These entries are backlinks only. A linked `tasks/todo` file is the editable source of truth when it exists. Closed or finding-canonical rows point at their review rule or campaign finding instead.

| Finding | Status | Canonical task |
|---------|--------|----------------|
| D0001 — Define complete ExternalName DNS cache identity and lifecycle | fixed (2026-09-03) | [resolution](../../../../skills/04-review/backends/regression-guards/externalname-dns-cache-identity-and-lifecycle.md) |
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
| D0019 — Make Webhook sliding windows time-coherent | resolved | [window coherence](../../../../skills/04-review/concurrency/regression-guards/sliding-window-check-then-increment.md) |
| D0020 — Refresh the shared LinkSys lifecycle documentation | needs-implementation | [campaign finding](D0020-link-sys-lifecycle-webhook-doc.md) |
| D0021 — Make partial LinkSys provider replacement authoritative | needs-discussion | [campaign finding](D0021-link-sys-cross-provider-partial-replacement.md) |
| D0022 — Refresh the LinkSys documentation set | needs-implementation | [campaign finding](D0022-refresh-link-sys-documentation-set.md) |
| D0023 — Define OTLP egress enforcement and safe endpoint identity | needs-investigation | [campaign finding](D0023-otlp-egress-safe-endpoint-identity.md) |
| D0024 — Unify Gateway request-template runtime policies | needs-discussion | [campaign finding](D0024-unify-gateway-request-template-runtime.md) |
| D0025 — Make GeoIP lookup failures follow failure policy | fixed | [campaign finding](D0025-geoip-lookup-failure-policy.md) |
| D0026 — Enforce EdgionAcme HTTP-01 gatewayRef ownership | needs-discussion | [campaign finding](D0026-acme-http01-gatewayref-ownership.md) |
| D0027 — Add ACME gatewayRef isolation integration coverage | closed (2026-09-06) | [review rule](../../../../skills/04-review/testing/intentional-design/acme-gatewayref-isolation-coverage-not-standalone.md) |
| D0028 — Define the Redis rate-limiter numeric domain | needs-discussion | [campaign finding](D0028-redis-rate-limiter-numeric-domain.md) |
| D0029 — Add Redis TokenBucket partial-refill integration coverage | needs-implementation | [campaign finding](D0029-redis-token-bucket-partial-refill-integration.md) |
| D0030 — Retain EdgionPlugins last-known-good only for the same resource identity | needs-discussion | [campaign finding](D0030-edgionplugins-last-known-good-resource-identity.md) |
| D0031 — Add EdgionPlugins LKG identity integration coverage | closed (2026-09-05) | [review rule](../../../../skills/04-review/testing/intentional-design/edgionplugins-lkg-identity-coverage-not-standalone.md) |
| D0033 — Refresh AllEndpointStatus outbound-header review knowledge | needs-implementation | [campaign finding](D0033-all-endpoint-status-review-knowledge.md) |
| D0034 — Fail closed on denied L7 named jumps | resolved | [campaign finding](D0034-l7-named-jump-denied-reference.md) |
| D0035 — Add borrowed Authorization header access | needs-discussion | [campaign finding](D0035-auth-borrowed-authorization-header.md) |
| D0036 — Audit auth credential-copy lifetimes | needs-discussion | [campaign finding](D0036-auth-credential-copy-lifetime-audit.md) |
| D0039 — Harden auth challenge realm quoted strings | needs-implementation | [campaign finding](D0039-auth-challenge-realm-quoted-string.md) |
| D0040 — Unify CORS validation and header precomputation | needs-implementation | [campaign finding](D0040-cors-validation-header-precomputation.md) |
| D0041 — Adjudicate CSRF duration-string migration | accepted-tradeoff | [review rule](../../../../skills/04-review/csrf/intentional-design/csrf-expires-integer-seconds-retained.md) |
| D0042 — Add CSRF Secret-rotation integration coverage | fixed (2026-09-06) | [review rule](../../../../skills/04-review/csrf/regression-guards/csrf-secret-rotation-integration-coverage.md) |
| D0043 — Enforce exactly one CtxSet transform operation | needs-implementation | [campaign finding](D0043-ctx-set-exact-transform-operation.md) |
| D0044 — Design sensitive ctx provenance and unmasking | needs-discussion | [campaign finding](D0044-ctx-sensitive-provenance-unmasking.md) |
| D0045 — Reject unsafe DynamicExternalUpstream SNI and Host overrides | fixed (2026-09-05) | [review rule](../../../../skills/04-review/tls/regression-guards/dynamic-external-upstream-sni-host-validation.md) |
| D0046 — Use canonical final HTTP status validation for local replies | needs-implementation | [campaign finding](D0046-final-http-status-validation-local-replies.md) |
| D0047 — Align HmacAuth documentation with the runtime wire format | needs-implementation | [campaign finding](D0047-hmac-auth-wire-format-documentation.md) |
| D0048 — Use one EdgionConfigData snapshot per authorization decision | resolved | [campaign finding](D0048-request-scoped-config-data-snapshot.md) |
| D0049 — Align IP restriction docs with named IpGroup schema | needs-implementation | [campaign finding](D0049-ip-restriction-named-group-documentation.md) |
| D0050 — Parse JWE protected headers as strict JSON objects | needs-implementation | [campaign finding](D0050-jwe-protected-header-strict-json.md) |
| D0051 — Align JWE resolved-credential failure policy | needs-discussion | [campaign finding](D0051-jwe-resolved-credential-auth-failure.md) |
| D0052 — Canonicalize auth claim-to-header projection | needs-discussion | [campaign finding](D0052-auth-claim-header-projection-plan.md) |
| D0053 — Enforce LDAP dial-time egress policy | resolved | [campaign finding](D0053-ldap-auth-dial-time-egress-policy.md) |
| D0055 — Use clone-cheap immutable local response bodies | needs-discussion | [campaign finding](D0055-clone-cheap-local-response-bodies.md) |
| D0056 — Enforce OpenID Connect dial-time egress policy | resolved | [campaign finding](D0056-openid-connect-dial-time-egress-policy.md) |
| D0057 — Add OpenID Connect egress integration coverage | pending | [campaign finding](D0057-openid-connect-egress-integration-coverage.md) |
| D0058 — Redact OpenID Connect dependency error responses | needs-implementation | [campaign finding](D0058-openid-connect-dependency-error-redaction.md) |
| D0059 — Coalesce and back off failed OpenID Connect discovery fetches | resolved | [campaign finding](D0059-openid-connect-discovery-failure-coalescing.md) |
| D0060 — Decide OpenID Connect session-cookie upstream ownership | resolved | [campaign finding](D0060-openid-connect-session-cookie-upstream-ownership.md) |
| D0061 — Remove the GeoIP updater test's host-temp-space assumption | resolved | [campaign finding](D0061-geoip-updater-test-temp-space.md) |
| D0062 — Upgrade Wasmtime for RUSTSEC-2026-0268 and RUSTSEC-2026-0269 | resolved | [campaign finding](D0062-upgrade-wasmtime-security-patches.md) |
| D0063 — Validate rate-limit response header names canonically | needs-implementation | [campaign finding](D0063-rate-limit-response-header-validation.md) |
| D0064 — Validate RealIp header names canonically | needs-implementation | [campaign finding](D0064-real-ip-header-name-validation.md) |
| D0066 — Bound RequestMirror concurrency and buffering | fixed (2026-09-08) | [resolution](../../../../skills/04-review/request-mirror/regression-guards/mirror-resource-budgets.md) |
| D0067 — Isolate RequestMirror unit tests from the network | needs-implementation | [campaign finding](D0067-request-mirror-unit-test-network-isolation.md) |
| D0068 — Unify response-header mutation policy | needs-discussion | [campaign finding](D0068-response-header-mutation-policy.md) |
| D0069 — Correct TraceContext documentation semantics | needs-implementation | [campaign finding](D0069-trace-context-documentation-semantics.md) |
| D0070 — Enforce TraceContext attachment cardinality | needs-discussion | [campaign finding](D0070-trace-context-attachment-cardinality.md) |
| D0071 — Reduce WAF header materialization | needs-investigation | [campaign finding](D0071-waf-header-materialization.md) |
| D0072 — Bound outbound HTTP/2 response header lists | needs-investigation | [campaign finding](D0072-outbound-http2-response-header-list-bound.md) |
| D0073 — Isolate Wasm host panics at VM boundaries | resolved | [campaign finding](D0073-wasm-host-panic-isolation.md) |
| D0079 — Correct PluginRuntime architecture documentation | needs-implementation | [campaign finding](D0079-plugin-runtime-architecture-documentation.md) |
| D0080 — Reduce per-chunk condition extraction cost | needs-investigation | [campaign finding](D0080-response-body-condition-extraction-cost.md) |
| D0081 — Fail closed on plugin IP matcher limit violations | needs-investigation | [campaign finding](D0081-plugin-ip-matcher-limit-activation.md) |
| D0082 — Fail closed on invalid AiGuard/AiProxy ordering | needs-investigation | [campaign finding](D0082-ai-guard-proxy-order-activation.md) |
| D0085 — Complete native Gateway API CORS validation parity | needs-implementation | [campaign finding](D0085-native-gateway-api-cors-validation-parity.md) |
| D0086 — Decide credentialed native CORS expose-headers wildcard semantics | needs-discussion | [campaign finding](D0086-credentialed-native-cors-expose-headers-wildcard.md) |
| D0087 — Add debug access-log wire integration coverage | resolved externally in `bfc2691e` | [maintained knowledge](../../../../skills/04-review/observability/regression-guards/debug-access-log-wire-and-lifecycle.md) |
| D0088 — Define a bounded debug access-log response surface | needs-discussion | [campaign finding](D0088-bounded-debug-access-log-response-surface.md) |
| D0089 — Fail closed on missing Gateway API ExtensionRef configuration | needs-implementation | [campaign finding](D0089-fail-closed-missing-extensionref-configuration.md) |
| D0090 — Complete Gateway API header-modifier validation and preparation parity | needs-implementation | [campaign finding](D0090-gateway-api-header-modifier-validation-preparation-parity.md) |
| D0091 — Unify proxied request-header mutation and framing policy | needs-discussion | [campaign finding](D0091-proxied-request-header-mutation-framing-policy.md) |
| D0092 — Complete Gateway API request-transform validation and preparation parity | needs-implementation | [campaign finding](D0092-gateway-api-request-transform-validation-preparation-parity.md) |
| D0093 — Correct Gateway API RequestRedirect documentation | needs-implementation | [campaign finding](D0093-correct-gateway-api-request-redirect-documentation.md) |
| D0094 — Reject DSL builtin calls beyond bytecode argument capacity | needs-implementation | [campaign finding](D0094-dsl-builtin-argument-count-encoding.md) |
| D0095 — Investigate plugin policy and invalid-generation retention | needs-investigation | [campaign finding](D0095-plugin-policy-invalid-generation-retention.md) |
| Precompile backend port resolution and fail closed on missing mappings | fixed (2026-09-08) | [resolution](../../../../skills/04-review/backends/regression-guards/named-port-resolve-against-originating-slice.md) |
| Fail closed on invalid Gateway BackendTLSPolicy candidates | completed | [architecture](../../../../skills/01-architecture/02-gateway/07-backends.md) |
| Make BackendTLSPolicy publication truly incremental | pending | [task](../../../todo/backend-tls-policy-true-incremental-publication.md) |
| Promote BackendTLSPolicy handling atomically to v1 only | resolved externally in `5cd384c6` | [maintained knowledge](../../../../skills/04-review/controller/regression-guards/backend-tls-policy-canonical-api-ingress.md) |
| Publish Service backend traffic policy through one immutable snapshot | pending | [task](../../../todo/backend-traffic-policy-atomic-service-snapshot.md) |
| Conf-Sync certificate watch content signature | accepted-tradeoff | [review rule](../../../../skills/04-review/conf-sync/intentional-design/conf-sync-cert-watch-mtime-length-signature.md) |
| Conf-Sync generic watch malformed event recovery | pending | [task](../../../todo/conf-sync-generic-watch-malformed-event-recovery.md) |
| Conf-Sync rotation transactional address selection | pending | [task](../../../todo/conf-sync-rotation-transactional-address-selection.md) |
| Fail closed on denied L4 backend references | completed | [review rule](../../../../skills/04-review/routing/regression-guards/l4-backend-ref-denied-fail-closed.md) |
| Denied backendRef HTTP/gRPC parity and grant hot-reload | pending | [task](../../../todo/denied-backend-ref-parity-and-grant-hot-reload.md) |
| Keep the load-balancer degrade-mode gauge current across lifecycle exits | pending | [task](../../../todo/lb-degrade-mode-metric-lifecycle.md) |
| Capture Service runtime health state once per load-balancer selection | completed | [review rule](../../../../skills/04-review/lb/regression-guards/service-runtime-snapshot-selection.md) |
| Run mTLS whitelist verification once for the leaf certificate | completed | [review rule](../../../../skills/04-review/tls/regression-guards/mtls-leaf-only-whitelist-verification.md) |
| Define typed mTLS allowed SAN semantics | pending | [task](../../../todo/mtls-typed-allowed-san-semantics.md) |
| SSE window strict residency and continuity | pending | [task](../../../todo/sse-window-strict-residency-and-continuity.md) |
| Retain TLS last-known-good only for the same resource identity | pending | [task](../../../todo/tls-last-known-good-resource-identity.md) |
| TLS `SslCtx` owned handoff and exactly-once audit | pending | [task](../../../todo/tls-ssl-ctx-owned-handoff-and-exactly-once-audit.md) |
| TLS store true incremental and atomic publication | pending | [task](../../../todo/tls-store-true-incremental-atomic-publication.md) |
| Unify Gateway request-template runtime policies | pending | [task](../../../todo/unify-gateway-request-template-runtime/unify-gateway-request-template-runtime.md) |
| Make GeoIP lookup failures follow failure policy | completed | [campaign finding](D0025-geoip-lookup-failure-policy.md) |
| Enforce EdgionAcme HTTP-01 gatewayRef ownership | needs-discussion | [campaign finding](D0026-acme-http01-gatewayref-ownership.md) |
| Add ACME gatewayRef isolation integration coverage | closed (2026-09-06) | [review rule](../../../../skills/04-review/testing/intentional-design/acme-gatewayref-isolation-coverage-not-standalone.md) |
| Define the Redis rate-limiter numeric domain | pending | [task](../../../todo/define-redis-rate-limiter-numeric-domain/define-redis-rate-limiter-numeric-domain.md) |
| Add Redis TokenBucket partial-refill integration coverage | pending | [task](../../../todo/add-redis-token-bucket-partial-refill-integration-coverage.md) |
| Retain EdgionPlugins last-known-good only for the same resource identity | needs-discussion | [campaign finding](D0030-edgionplugins-last-known-good-resource-identity.md) |
| Refresh AllEndpointStatus outbound-header review knowledge | pending | [task](../../../todo/refresh-all-endpoint-status-header-review-records.md) |
| Fail closed on denied L7 named jumps | completed | [review rule](../../../../skills/04-review/routing/regression-guards/denied-named-jump-fails-closed.md) |
| Add denied named-jump integration coverage | pending | [task](../../../todo/add-denied-named-jump-integration-coverage.md) |
| Add borrowed Authorization header access | pending | [task](../../../todo/add-borrowed-authorization-header-access.md) |
| Audit auth credential-copy lifetimes | pending | [task](../../../todo/audit-auth-credential-copy-lifetimes.md) |
| Harden auth challenge realm quoted strings | pending | [task](../../../todo/harden-auth-challenge-realm-quoted-strings.md) |
| Unify CORS validation and header precomputation | pending | [task](../../../todo/unify-cors-validation-and-header-precomputation.md) |
| Adjudicate CSRF duration-string migration | accepted-tradeoff | [review rule](../../../../skills/04-review/csrf/intentional-design/csrf-expires-integer-seconds-retained.md) |
| Add CSRF Secret-rotation integration coverage | fixed (2026-09-06) | [review rule](../../../../skills/04-review/csrf/regression-guards/csrf-secret-rotation-integration-coverage.md) |
| Enforce exactly one CtxSet transform operation | pending | [task](../../../todo/enforce-one-ctx-set-transform-operation.md) |
| Design sensitive ctx provenance and unmasking | pending | [task](../../../todo/design-ctx-sensitive-provenance-unmasking.md) |
| Reject unsafe DynamicExternalUpstream SNI and Host overrides | fixed (2026-09-05) | [review rule](../../../../skills/04-review/tls/regression-guards/dynamic-external-upstream-sni-host-validation.md) |
| Use canonical final HTTP status validation for local replies | pending | [task](../../../todo/use-canonical-final-http-status-validation-for-local-replies.md) |
| Align HmacAuth documentation with the runtime wire format | completed | [campaign finding](D0047-hmac-auth-wire-format-documentation.md) |
| Use one EdgionConfigData snapshot per authorization decision | completed | [campaign finding](D0048-request-scoped-config-data-snapshot.md) |
| Align IP restriction docs with named IpGroup schema | pending | [task](../../../todo/align-ip-restriction-named-group-documentation.md) |
| Harden JWE protected header JSON parsing | pending | [task](../../../todo/harden-jwe-protected-header-json-parsing.md) |
| Align JWE resolved-credential failure policy | pending | [task](../../../todo/align-jwe-resolved-credential-failure-policy.md) |
| Canonicalize auth claim-to-header projection | pending | [task](../../../todo/canonicalize-auth-claim-header-projection.md) |
| Enforce LDAP dial-time egress policy | completed | [campaign finding](D0053-ldap-auth-dial-time-egress-policy.md) |
| Use clone-cheap immutable local response bodies | pending | [task](../../../todo/use-clone-cheap-local-response-bodies.md) |
| Enforce OpenID Connect dial-time egress policy | completed | [campaign finding](D0056-openid-connect-dial-time-egress-policy.md) |
| Add OpenID Connect egress integration coverage | pending | [task](../../../todo/add-openid-connect-egress-integration-coverage.md) |
| Redact OpenID Connect dependency error responses | pending | [task](../../../todo/redact-openid-connect-dependency-error-responses.md) |
| Coalesce and back off failed OpenID Connect discovery fetches | completed | [campaign finding](D0059-openid-connect-discovery-failure-coalescing.md) |
| Decide OpenID Connect session-cookie upstream ownership | completed | [campaign finding](D0060-openid-connect-session-cookie-upstream-ownership.md) |
| Remove the GeoIP updater test's host-temp-space assumption | completed | [campaign finding](D0061-geoip-updater-test-temp-space.md) |
| Upgrade Wasmtime for RUSTSEC-2026-0268 and RUSTSEC-2026-0269 | completed | [campaign finding](D0062-upgrade-wasmtime-security-patches.md) |
| Validate rate-limit response header names canonically | pending | [task](../../../todo/validate-rate-limit-response-header-names.md) |
| Validate RealIp header names canonically | pending | [task](../../../todo/validate-real-ip-header-names.md) |
| Verify RegionRoute validation across hot reload and management APIs | resolved externally in `2c01a075` | [maintained knowledge](../../../../skills/04-review/routing/regression-guards/region-route-effective-table-validation.md) |
| Isolate RequestMirror unit tests from the network | pending | [task](../../../todo/isolate-request-mirror-unit-tests-from-network.md) |
| Unify response-header mutation policy | pending | [task](../../../todo/unify-response-header-mutation-policy.md) |
| Correct TraceContext documentation semantics | pending | [task](../../../todo/correct-trace-context-documentation-semantics.md) |
| Enforce TraceContext attachment cardinality | pending | [task](../../../todo/enforce-trace-context-attachment-cardinality.md) |
| Reduce WAF header materialization | pending | [task](../../../todo/reduce-waf-header-materialization.md) |
| Bound outbound HTTP/2 response header lists | pending | [task](../../../todo/bound-outbound-http2-response-header-lists.md) |
| Isolate Wasm host panics at VM boundaries | completed | [task](../../../todo/isolate-wasm-host-panics-at-vm-boundaries.md) |
| Offload Wasm OCI manifest parsing and archive extraction (D0075) | completed externally in 256733d7 | [maintained knowledge](../../../../skills/04-review/wasm/intentional-design/bounded-oci-artifact-memory-cache.md#manifest-and-archive-cpu-boundary-d0075) |
| Fix Wasm shared-data gauge generation identity and lifecycle | pending | [task](../../../todo/wasm-shared-data-gauge-generation-lifecycle.md) |
| Redact and bound Wasm URL source identity | pending | [task](../../../todo/wasm-source-url-label-and-error-redaction.md) |
| Emit bounded diagnostics for outbound warnIfMissing fields | pending | [task](../../../todo/outbound-warn-if-missing-diagnostics.md) |
| Correct PluginRuntime architecture documentation | pending | [task](../../../todo/correct-plugin-runtime-architecture-documentation.md) |
| Reduce per-chunk response-body condition extraction cost | pending | [task](../../../todo/reduce-response-body-condition-extraction-cost.md) |
| Fail closed on plugin IP matcher limit violations | pending | [task](../../../todo/fail-closed-plugin-ip-matcher-limit-violations.md) |
| Fail closed on invalid AiGuard/AiProxy ordering | pending | [task](../../../todo/fail-closed-ai-guard-proxy-order-violations.md) |
| Complete native Gateway API CORS validation parity | pending | [task](../../../todo/complete-native-gateway-api-cors-validation-parity.md) |
| Decide credentialed native CORS expose-headers wildcard semantics | pending | [task](../../../todo/decide-credentialed-native-cors-expose-headers-wildcard.md) |
| Add debug access-log wire integration coverage | resolved externally in `bfc2691e` | [maintained knowledge](../../../../skills/04-review/observability/regression-guards/debug-access-log-wire-and-lifecycle.md) |
| Define a bounded debug access-log response surface | pending | [task](../../../todo/define-bounded-debug-access-log-response-surface.md) |
| Fail closed on missing Gateway API ExtensionRef configuration | pending | [task](../../../todo/fail-closed-missing-gateway-api-extensionref-configuration.md) |
| Complete Gateway API header-modifier validation and preparation parity | pending | [task](../../../todo/complete-gateway-api-header-modifier-validation-and-preparation-parity.md) |
| Unify proxied request-header mutation and framing policy | pending | [task](../../../todo/unify-proxied-request-header-mutation-and-framing-policy.md) |
| Complete Gateway API request-transform validation and preparation parity | pending | [task](../../../todo/complete-gateway-api-request-transform-validation-and-preparation-parity.md) |
| Correct Gateway API RequestRedirect documentation | pending | [task](../../../todo/correct-gateway-api-request-redirect-documentation.md) |
| Reject DSL builtin calls beyond bytecode argument capacity | pending | [task](../../../todo/reject-dsl-builtin-calls-beyond-argument-capacity.md) |
| Investigate plugin policy and invalid-generation retention | pending | [task](../../../todo/investigate-plugin-policy-invalid-generation-retention.md) |
| Align upstream response stage documentation | pending | [task](../../../todo/align-upstream-response-stage-documentation.md) |
| Align response-body filter terminal documentation | pending | [task](../../../todo/align-response-body-filter-terminal-documentation.md) |
| Avoid Wasm response in-flight cache-key allocation | pending | [task](../../../todo/avoid-wasm-response-inflight-cache-key-allocation.md) |
| Unify terminal local-reply status validation | pending | [task](../../../todo/unify-terminal-local-reply-status-validation.md) |
| Add native WAF response-stage invalid-status integration coverage | resolved (2026-09-09) | [maintained knowledge](../../../../skills/04-review/http-body/regression-guards/response-stage-denial-must-terminate.md) |
| Unify response Content-Encoding classification | pending | [task](../../../todo/unify-response-content-encoding-classification.md) |
| Define backendRef upstream-selection plugin semantics | pending | [task](../../../todo/define-backendref-upstream-selection-plugin-semantics.md) |
| Add deferred-credential Connection nomination integration coverage | resolved (2026-09-09) | [maintained knowledge](../../../../skills/04-review/security/regression-guards/deferred-credential-connection-nomination.md) |
| Clarify redirect connection policy | pending | [task](../../../todo/clarify-redirect-connection-policy.md) |
| Add session cookie field integration coverage | resolved (2026-09-09) | [maintained knowledge](../../../../skills/04-review/routing/regression-guards/session-persistence-token-extraction.md) |
| Add L4 duplex backpressure integration coverage | resolved (2026-09-09) | [maintained knowledge](../../../../skills/04-review/routing/regression-guards/l4-duplex-relay-centralization.md) |
| Reconcile TLSRoute version guidance | pending | [task](../../../todo/reconcile-tlsroute-version-guidance.md) |
| Reconcile route architecture ownership and rebuild documentation | pending | [task](../../../todo/route-architecture-manager-cache-documentation.md) |
| Pre-resolve L4 listener timeouts | pending | [task](../../../todo/pre-resolve-l4-listener-timeouts.md) |
| Refresh L4 connect-timeout knowledge | pending | [task](../../../todo/refresh-l4-connect-timeout-knowledge.md) |
| Scope TLSRoute effective hostnames to attachments | pending (high) | [task](../../../todo/scope-tlsroute-effective-hostnames-to-attachments.md) |
| Remove TLSRoute Stage-1 fallback coherently | pending | [task](../../../todo/remove-tlsroute-stage1-fallback-coherently.md) |
| Apply sensitive mode to TLS log surfaces | pending | [task](../../../todo/apply-sensitive-mode-to-tls-log-surfaces.md) |
| Investigate TLS setup write deadlines | needs-investigation | [task](../../../todo/investigate-tls-setup-write-deadlines.md) |
| Add malformed ClientHello passthrough coverage (D0118) | pending | [task](../../../todo/add-malformed-clienthello-passthrough-coverage.md) |
| Reconcile HTTP context layout expectations (D0119) | resolved externally in `2c01a075` | [campaign resolution](D0119-http-context-layout-expectations.md) |
| Retire UDP sessions by generation (D0120) | pending | [task](../../../todo/retire-udp-sessions-by-generation.md) |
| Validate UDP upstream response source (D0121) | pending | [task](../../../todo/validate-udp-upstream-response-source.md) |
| Wire UDP listener process shutdown (D0122) | pending | [task](../../../todo/wire-udp-listener-process-shutdown.md) |
| Replace Gateway generations on coalesced add (D0123) | pending | [task](../../../todo/replace-gateway-generations-on-coalesced-add.md) |
| Reconcile Gateway frontend TLS knowledge (D0124) | pending | [task](../../../todo/reconcile-gateway-frontend-tls-knowledge.md) |
| Investigate Gateway TLS collision retention (D0125) | needs-investigation | [task](../../../todo/investigate-gateway-tls-collision-snapshot-retention.md) |
| Enforce canonical gRPC terminal status headers (D0126) | pending | [task](../../../todo/enforce-canonical-grpc-terminal-status-headers.md) |
| Reject empty individual outbound CA bundles (D0127) | pending | [task](../../../todo/reject-empty-individual-outbound-ca-bundles.md) |
| Bound and redact DSL Redis error results | pending | [task](../../../todo/bound-and-redact-dsl-redis-error-results.md) |
| Avoid DSL Redis link-key clones | pending | [task](../../../todo/avoid-dsl-redis-link-key-clones.md) |
| Isolate shared outbound-call library unit tests from network (D0128) | pending | [task](../../../todo/isolate-shared-outbound-call-unit-tests-from-network.md) |
