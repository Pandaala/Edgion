# Architecture optimization review

Status: in progress. Production source is read-only.

Workflow: `edgion-architecture-review` (personal skill).
See [plan](PLAN.md), [coverage](COVERAGE.md), [state](STATE.json) and [baseline](BASELINE.json).

Source reviewed: 512 / 1676; 467 primary partitions.

| ID | Status | Kind | Optimization |
| --- | --- | --- | --- |
| ARCH-001 | resolved | architecture-debt | [Closed feature-operation coupling finding; acceptance evidence](TASK-ACCEPTANCE-AI-20261011-G.json) |
| ARCH-002 | resolved | architecture-debt | [Closed request-body contract ownership finding; acceptance evidence](TASK-ACCEPTANCE-ARCH-002-20261011.json) |
| ARCH-003 | resolved | architecture-debt | [Closed terminal gRPC header preparation finding; acceptance evidence](TASK-ACCEPTANCE-ARCH-003-20261012.json) |
| ARCH-004 | resolved | architecture-debt | [Closed context query-cache source ownership finding; acceptance evidence](TASK-ACCEPTANCE-ARCH-004-20261012.json) |
| ARCH-005 | resolved | architecture-debt | [Closed typed transport admission finding; acceptance evidence](TASK-ACCEPTANCE-ARCH-005-20261012.json) |
| ARCH-006 | resolved | architecture-debt | [Closed resolved logical body-limit finding; acceptance evidence](TASK-ACCEPTANCE-ARCH-006-20261012.json) |
| ARCH-007 | candidate | architecture-debt | [Match mTLS conditions through borrowed certificate candidates](ARCH-007-borrowed-mtls-condition-candidates.md) |
| ARCH-008 | candidate | architecture-debt | [Borrow precompiled AI backend lookup keys during request planning](ARCH-008-precompiled-ai-backend-lookup-key.md) |
| ARCH-009 | candidate | architecture-debt | [Keep credential-admission projection inside the prepared AI attempt](ARCH-009-prepared-ai-admission-input-owner.md) |
| ARCH-010 | candidate | behavior-defect | [Restrict the shared AI JSON parser to JSON whitespace](ARCH-010-json-span-format-whitespace.md) |
| ARCH-011 | candidate | behavior-defect | [Give AiGuard composition ownership of the final replacement budget](ARCH-011-ai-guard-final-replacement-budget.md) |
| ARCH-012 | candidate | behavior-defect | [Own native JSON UTF-8 admission independently of observation](ARCH-012-native-json-utf8-admission-owner.md) |
| ARCH-013 | candidate | behavior-defect | [Preserve typed SSE classification at canonical semantic handoff](ARCH-013-canonical-sse-block-classification.md) |
| ARCH-014 | candidate | architecture-debt | [Remove the complete GatewayClass mirror maintained only for diagnostics](ARCH-014-gateway-class-diagnostic-mirror.md) |
| ARCH-015 | confirmed | behavior-defect | [Bind persisted GeoIP validators to their update source](ARCH-015-geoip-validator-source-identity.md) |
| ARCH-016 | candidate | architecture-debt | [Compile BackendSelector as immutable generation-owned state](ARCH-016-immutable-backend-ref-selector.md) |
| ARCH-017 | candidate | architecture-debt | [Prepare immutable native backend TLS components with their publishing generation](ARCH-017-prepared-native-backend-tls-generation.md) |
| ARCH-018 | confirmed | architecture-debt | [Retire the unused socket-hash outlier execution owner](ARCH-018-outlier-production-identity-owner.md) |
| ARCH-019 | candidate | architecture-debt | [Borrow resolved GeoIP dimensions when projecting an access log](ARCH-019-borrowed-geo-access-log-projection.md) |
| ARCH-020 | confirmed | architecture-debt | [Normalize discovery geometry without constructing provisional runtime selectors](ARCH-020-discovery-normalized-geometry-owner.md) |
| ARCH-021 | candidate | architecture-debt | [Keep ready Service endpoints numeric across the shared outbound boundary](ARCH-021-numeric-outbound-service-endpoints.md) |
| ARCH-022 | candidate | architecture-debt | [Keep health status markers without a second threshold engine](ARCH-022-active-health-marker-only-store.md) |
| ARCH-023 | candidate | architecture-debt | [Share prepared policy ownership across the target index](ARCH-023-shared-prepared-policy-target-index.md) |
| ARCH-024 | candidate | architecture-debt | [Normalize EBTP deltas against previous and final key ownership](ARCH-024-ebtp-final-key-delta-normalization.md) |
| ARCH-025 | candidate | behavior-defect | [Reject malformed Elasticsearch date formats before worker publication](ARCH-025-es-date-pattern-cold-validation.md) |
| ARCH-026 | candidate | behavior-defect | [Compare LocalFile writer attachment with the current active file](ARCH-026-local-file-active-attachment-identity.md) |
| ARCH-027 | candidate | architecture-debt | [Bound Elasticsearch bulk acknowledgement reception and projection](ARCH-027-bounded-es-bulk-acknowledgement.md) |
| ARCH-028 | confirmed | architecture-debt | [Evaluate inline and referenced IP matchers through one borrowed union](ARCH-028-borrowed-ip-matcher-union.md) |
| ARCH-029 | candidate | architecture-debt | [Reuse the canonical body-error access-status classifier in FormJsonTransform](ARCH-029-form-body-error-classification.md) |
| ARCH-030 | candidate | architecture-debt | [Give canonical bounded regex output amortized capacity growth](ARCH-030-bounded-regex-output-growth.md) |
| ARCH-031 | candidate | architecture-debt | [Retire the unused JSON reader and test the current bounded OIDC path](ARCH-031-retire-orphan-json-reader.md) |
| ARCH-032 | candidate | architecture-debt | [Prepare the buffered ExtProc body message after its headers verdict](ARCH-032-extproc-deferred-buffered-message.md) |
| ARCH-033 | candidate | architecture-debt | [Allocate transcoder callback output only when a streaming message produces it](ARCH-033-lazy-transcoder-callback-writer.md) |
| ARCH-034 | candidate | architecture-debt | [Keep the decoded DER owner in Base64 header certificate authentication](ARCH-034-header-cert-leaf-der-owner.md) |
| ARCH-035 | candidate | architecture-debt | [Prepare the OIDC signing algorithm policy per plugin generation](ARCH-035-oidc-prepared-signing-policy.md) |
| ARCH-036 | candidate | behavior-defect | [Use the authentication failure contract when HMAC has no usable credentials](ARCH-036-hmac-no-credential-auth-failure.md) |
| ARCH-037 | candidate | behavior-defect | [Substitute ProxyRewrite path parameters at their matched token boundaries](ARCH-037-proxy-rewrite-token-boundaries.md) |
| ARCH-038 | candidate | architecture-debt | [Snapshot only the requested Wasm ABI buffer range](ARCH-038-wasm-range-buffer-snapshot.md) |
| ARCH-039 | candidate | architecture-debt | [Release host phase payloads before returning a healthy Wasm VM to its pool](ARCH-039-wasm-idle-host-payload-release.md) |
| ARCH-040 | candidate | behavior-defect | [Pass the resolved HTTPS HTTP/2 flag to the TLS settings owner](ARCH-040-https-http2-alpn-flag.md) |
| ARCH-041 | candidate | behavior-defect | [Validate shared HTTP listener startup declarations before deduplicating](ARCH-041-shared-http-startup-policy.md) |
| ARCH-042 | candidate | architecture-debt | [Materialize only the requested SAN projection](ARCH-042-san-output-projection.md) |
| ARCH-043 | confirmed | behavior-defect | [Give cached Gateway request metrics an eviction-safe lifetime](ARCH-043-cached-metric-handle-lifetime.md) |
| ARCH-044 | candidate | behavior-defect | [Preserve the declared Pingora process CLI overrides during configuration merge](ARCH-044-server-cli-merge.md) |
| ARCH-045 | candidate | architecture-debt | [Retire the unused route inventories beside the published gRPC match engine](ARCH-045-grpc-unused-published-inventories.md) |
| ARCH-046 | candidate | behavior-defect | [Preserve the cause of UDP session creation failure](ARCH-046-udp-session-creation-failure-cause.md) |
| ARCH-047 | confirmed | behavior-defect | [Read resource kind from the structured top-level envelope](ARCH-047-structured-resource-kind-extraction.md) |
| ARCH-048 | candidate | architecture-debt | [Remove transient heap strings from the canonical query percent decoder](ARCH-048-query-percent-token-allocation.md) |
| ARCH-049 | candidate | behavior-defect | [Make the startup writability probe own its file exclusively](ARCH-049-exclusive-work-dir-probe.md) |
| ARCH-050 | candidate | architecture-debt | [Reuse request-entry configuration validation within one resource-validation invocation](ARCH-050-invocation-local-plugin-validation.md) |
| ARCH-051 | candidate | architecture-debt | [Retire the orphan Webhook authentication parsing mechanism](ARCH-051-retired-webhook-auth-resolvers.md) |
| ARCH-052 | candidate | architecture-debt | [Avoid whole-path copies in unchanged normalization stages](ARCH-052-normalization-stage-buffer-ownership.md) |
| ARCH-053 | candidate | architecture-debt | [ARCH-053: Preserve cold-resolved GraphQL cost-rule identity in the compiled field model](ARCH-053-cold-resolved-graphql-cost-rules.md) |
| ARCH-054 | candidate | architecture-debt | [Borrow unescaped gRPC transcoding mapping inputs](ARCH-054-borrow-transcoder-mapping-inputs.md) |
| ARCH-055 | candidate | architecture-debt | [Preserve full policy precedence across status seeding and conf-sync](ARCH-055-policy-precedence-transport-precision.md) |
| ARCH-056 | confirmed | architecture-debt | [Retire the L4 plugin's obsolete HTTP-schema migration detector](ARCH-056-stream-legacy-schema-detector.md) |
| ARCH-057 | candidate | behavior-defect | [Preserve operator key presence when decoding outbound success predicates](ARCH-057-success-predicate-key-presence.md) |
| ARCH-058 | candidate | architecture-debt | [Retire the disconnected EdgionTls hostname matcher](ARCH-058-retire-resource-tls-hostname-matcher.md) |
| ARCH-059 | candidate | architecture-debt | [Reuse the existing Kubernetes name predicates for health-check references](ARCH-059-health-reference-name-validation.md) |
| ARCH-060 | candidate | architecture-debt | [Resolve session persistence lifetime with the publishing rule generation](ARCH-060-cold-session-persistence-lifetime.md) |
| ARCH-061 | candidate | behavior-defect | [Reject multiple custom-field source keys before typed selection](ARCH-061-custom-field-source-arity.md) |
| ARCH-062 | candidate | architecture-debt | [Borrow HTTPRoute matches for prefix-transform validation](ARCH-062-borrow-prefix-transform-matches.md) |
| ARCH-063 | confirmed | architecture-debt | [Retire the orphan ConfigData overlay pipeline while keeping the live borrowed owner](ARCH-063-orphan-config-data-overlay.md) |
| ARCH-064 | candidate | behavior-defect | [Reject malformed AllEndpointStatus method overrides before publication](ARCH-064-all-endpoint-status-method-preparation.md) |
| ARCH-065 | candidate | behavior-defect | [Align Kafka linger admission with its existing producer deadline](ARCH-065-kafka-linger-admission-parity.md) |
| ARCH-066 | candidate | behavior-defect | [Keep raw Wasm callout origins out of configuration diagnostics](ARCH-066-wasm-callout-origin-diagnostic-redaction.md) |
| ARCH-067 | candidate | architecture-debt | [Borrow CtxSet transform fields during configuration validation](ARCH-067-borrow-ctx-set-transform-validation.md) |
| ARCH-068 | candidate | behavior-defect | [Validate ExtProc targets against the URI the cold client actually constructs](ARCH-068-ext-proc-target-uri-admission.md) |
| ARCH-069 | candidate | architecture-debt | [Use the strict JSON result object as its duplicate-key owner](ARCH-069-strict-json-single-key-owner.md) |
| ARCH-070 | candidate | behavior-defect | [Reject HTTP-invalid configured authentication header names during shared admission](ARCH-070-authentication-header-name-admission.md) |
| ARCH-071 | candidate | architecture-debt | [Borrow ConfigData fields when checking prepared IpList topology](ARCH-071-borrow-ip-list-topology-comparison.md) |
| ARCH-072 | candidate | architecture-debt | [Retain ProxyRewrite compilation results from one cold preparation pass](ARCH-072-proxy-rewrite-single-preparation.md) |
| ARCH-073 | candidate | behavior-defect | [Clamp per-plugin estimator capacity before an overflowing unit conversion](ARCH-073-local-estimator-overflow-before-cap.md) |
| ARCH-074 | confirmed | architecture-debt | [Retire the legacy singular TLS owner from the shared channel-security contract](ARCH-074-canonical-channel-security-config.md) |
| ARCH-075 | candidate | behavior-defect | [Bind conf_sync identity validation to the certificate installed by the transport](ARCH-075-conf-sync-installed-leaf-validation.md) |
| ARCH-076 | confirmed | behavior-defect | [ARCH-076 — Strip configured HTTP DNS URLs before transport errors become diagnostics](ARCH-076-httpdns-transport-error-url-redaction.md) |
| ARCH-077 | confirmed | architecture-debt | [ARCH-077 — Prepare immutable HTTP DNS response metadata with its configuration generation](ARCH-077-httpdns-prepared-response-metadata.md) |
| ARCH-078 | confirmed | behavior-defect | [Preserve process-owned degraded state across metric idle eviction](ARCH-078-system-state-metric-lifetime.md) |
| ARCH-079 | candidate | architecture-debt | [ARCH-079: Disarm Etcd lock fallback cleanup after successful explicit release](ARCH-079-etcd-lock-completion.md) |
| ARCH-080 | confirmed | behavior-defect | [ARCH-080: Resolve the Controller's explicit plaintext override before auto-CA lifecycle setup](ARCH-080-controller-auto-ca-plaintext-selection.md) |
| ARCH-081 | candidate | behavior-defect | [ARCH-081: Keep leader diagnostics outside the post-win supervision boundary](ARCH-081-etcd-leader-diagnostic-supervision.md) |
| ARCH-082 | confirmed | behavior-defect | [Project successful federation watch metrics onto the canonical kind domain](ARCH-082-successful-fed-watch-kind-label.md) |
| ARCH-083 | confirmed | behavior-defect | [Pass the current federation snapshot to each supervised client start](ARCH-083-federation-restart-current-snapshot.md) |
| ARCH-084 | confirmed | behavior-defect | [Cancel session-owned federation watch tasks when the connection future is dropped](ARCH-084-federation-session-drop-cancellation.md) |
| ARCH-085 | candidate | behavior-defect | [Validate advertised Etcd leader IPs semantically instead of comparing spellings](ARCH-085-etcd-advertised-address-classification.md) |
| ARCH-086 | confirmed | behavior-defect | [ARCH-086: Preserve invalid UTF-8 documents through the etcd ingress lifecycle](ARCH-086-etcd-invalid-utf8-ingress-owner.md) |
| ARCH-087 | confirmed | behavior-defect | [Create the cache watch notification future before inspecting the event store](ARCH-087-cache-watch-notification-registration.md) |
| ARCH-088 | confirmed | behavior-defect | [ARCH-088: Give active-controller accounting the controller future's lifetime](ARCH-088-active-controller-scope-accounting.md) |
| ARCH-089 | candidate | architecture-debt | [Give all schema-validation diagnostics the existing bounded value-free formatter](ARCH-089-unified-safe-schema-diagnostics.md) |
| ARCH-090 | confirmed | architecture-debt | [ARCH-090: Own bounded status-entry retention during snapshot collection](ARCH-090-bounded-status-entry-retention.md) |
| ARCH-091 | confirmed | architecture-debt | [ARCH-091: Reuse the call-local raw YAML view at resource-ingress preparation](ARCH-091-call-local-resource-yaml-view.md) |
| ARCH-092 | confirmed | behavior-defect | [ARCH-092: Tear down a ready local configuration generation when its backend error channel closes unexpectedly](ARCH-092-closed-backend-error-channel-teardown.md) |
| ARCH-093 | confirmed | behavior-defect | [ARCH-093: Deliver the complete FS initialization before publishing readiness](ARCH-093-fs-initial-delivery-backpressure.md) |
| ARCH-094 | candidate | behavior-defect | [Keep CredentialManager death classification and its Gauge write under one service lock](ARCH-094-credential-observer-locked-gauge.md) |
| ARCH-096 | candidate | behavior-defect | [ARCH-096: Remove the previous raw generation when the FS worker cannot read its source](ARCH-096-fs-read-error-raw-generation.md) |
| ARCH-098 | candidate | behavior-defect | [ARCH-098: Bind DNS slice publication identity to the port fields the Gateway consumes](ARCH-098-dns-published-port-identity.md) |
| ARCH-099 | candidate | architecture-debt | [ARCH-099: Project only a target's policy timestamps under the existing snapshot lock](ARCH-099-target-local-policy-timestamps.md) |
| ARCH-100 | candidate | behavior-defect | [ARCH-100: Reject invalid LabelSelector expression cardinality before namespace matching](ARCH-100-namespace-selector-expression-cardinality.md) |
| ARCH-103 | candidate | architecture-debt | [ARCH-103: Prune empty ConfigData reference buckets when replacing an owner's edges](ARCH-103-config-data-empty-reference-buckets.md) |
| ARCH-104 | candidate | behavior-defect | [ARCH-104: Apply CredentialSource egress policy to literal issuer hosts](ARCH-104-oauth-issuer-literal-egress.md) |
| ARCH-105 | candidate | behavior-defect | [ARCH-105: Bind local credential-manager results to the source snapshot that produced them](ARCH-105-credential-result-source-ownership.md) |
| ARCH-106 | candidate | behavior-defect | [Enforce policy ancestor capacity after protected history and owned entries are composed](ARCH-106-policy-ancestor-final-capacity.md) |
| ARCH-108 | candidate | architecture-debt | [ARCH-108: Validate request-template Secret status without collecting discarded values](ARCH-108-request-secret-status-value-projection.md) |
| ARCH-110 | candidate | architecture-debt | [Maintain the ReferenceGrant namespace index from the changed grant](ARCH-110-reference-grant-local-index-updates.md) |
| ARCH-111 | confirmed | behavior-defect | [Apply existing watch selection to EndpointSlice creation](ARCH-111-endpoint-slice-watch-selection.md) |
| ARCH-112 | candidate | behavior-defect | [Keep the active renewal deadline effective during election IO](ARCH-112-kubernetes-active-renewal-deadline.md) |
| ARCH-117 | confirmed | behavior-defect | [Publish the handshake verifier and its revocation token as one snapshot](ARCH-117-conf-sync-handshake-trust-snapshot.md) |
| ARCH-118 | confirmed | behavior-defect | [Bind gateway presence to live metadata registrations](ARCH-118-metadata-registration-presence-owner.md) |
| ARCH-119 | confirmed | behavior-defect | [Keep metadata observation tied to the last delivered count](ARCH-119-metadata-delivered-count-observation.md) |
| ARCH-125 | candidate | behavior-defect | [Correlate each BackendTLSPolicy CA reference with its resolved material identity](ARCH-125-backend-tls-ca-reference-material-binding.md) |
| ARCH-130 | confirmed | behavior-defect | [Bind per-replica identity publication to a snapshot captured before the CA read](ARCH-130-identity-sync-conditional-publication.md) |
| ARCH-135 | candidate | architecture-debt | [Reuse stateless validation for TLS-route IP restriction preparse](ARCH-135-tls-route-ip-stateless-preparse.md) |
| ARCH-136 | candidate | behavior-defect | [Preserve missing server material errors in EdgionTls status projection](ARCH-136-tls-status-missing-server-material.md) |
| ARCH-140 | confirmed | behavior-defect | [Keep authored values out of the Admin resource parser's diagnostics](ARCH-140-admin-parser-value-free-diagnostics.md) |
| ARCH-141 | candidate | behavior-defect | [Omit the ACME contact email from account-registration logs](ARCH-141-acme-contact-email-log-omission.md) |
| ARCH-146 | confirmed | behavior-defect | [Mutate only the canonical RegionRouteOverride table during failover](ARCH-146-region-failover-canonical-table.md) |
| ARCH-151 | candidate | architecture-debt | [Reuse the existing Gateway frontend structural-validation owner](ARCH-151-gateway-frontend-validation-owner.md) |
| ARCH-152 | candidate | behavior-defect | [Reject unrepresentable Gateway listener ports before deriving socket identity](ARCH-152-gateway-listener-port-admission.md) |
| ARCH-156 | candidate | behavior-defect | [Prove a semantic path boundary for delegated regular expressions](ARCH-156-delegated-regex-semantic-boundary.md) |
| ARCH-157 | candidate | architecture-debt | [Keep OIDC material semantics out of the generic Secret lifecycle](ARCH-157-secret-oidc-material-ownership.md) |
| ARCH-171 | candidate | architecture-debt | [Borrow the parsed CLI page while projecting metadata rows](ARCH-171-cli-page-metadata-borrow.md) |
| ARCH-176 | candidate | behavior-defect | [Keep cache namespace filtering in the paginated get path](ARCH-176-cli-cache-namespace-pagination.md) |
| ARCH-177 | candidate | behavior-defect | [Propagate directory apply failures through the existing batch result](ARCH-177-cli-directory-apply-outcome.md) |
| ARCH-178 | candidate | behavior-defect | [ARCH-178 — Give CLI connection diagnostics one safe URL projection](ARCH-178-cli-diagnostic-url-owner.md) |
| ARCH-181 | candidate | behavior-defect | [Compile mapped IPv6 CIDRs into the address family used by the unified matcher](ARCH-181-mapped-cidr-family-admission.md) |
| ARCH-186 | candidate | architecture-debt | [Move the retiring LRU node key instead of cloning it](ARCH-186-lru-retiring-key-ownership.md) |
| ARCH-201 | confirmed | architecture-debt | [Borrow unchanged XML token text before constructing its owned node](ARCH-201-xml-decoded-scratch-owner.md) |
| ARCH-206 | confirmed | behavior-defect | [ARCH-206 — Keep compiler-owned loop temporaries outside source name resolution](ARCH-206-compiler-private-loop-slots.md) |
| ARCH-211 | confirmed | behavior-defect | [Charge map key expressions to the parser's existing AST budget](ARCH-211-parser-map-key-node-budget.md) |
| ARCH-226 | candidate | architecture-debt | [ARCH-226: Borrow unchanged form fields before publishing owned WAF body entries](ARCH-226-waf-form-decoder-scratch.md) |
| ARCH-227 | candidate | architecture-debt | [ARCH-227: Commit only surviving transformation indices in the semantic fold](ARCH-227-waf-semantic-reset-output.md) |
| ARCH-231 | candidate | behavior-defect | [Account canonical WAF arena capacity before publishing the generation](ARCH-231-waf-canonical-arena-capacity.md) |
| ARCH-232 | candidate | architecture-debt | [Share immutable request-head bytes across WAF collection aliases](ARCH-232-waf-request-head-alias-bytes.md) |
| ARCH-252 | confirmed | architecture-debt | [Retire the orphan file-based LB verification surface](ARCH-252-orphan-lb-log-analyzer.md) |
| ARCH-260 | candidate | architecture-debt | [TestService regeneration has no declared input-schema owner](ARCH-260-test-service-schema-regeneration-owner.md) |
| ARCH-264 | candidate | behavior-defect | [ARCH-264 — Dynamic scenarios must own initial state and restoration](ARCH-264-dynamic-scenario-state-restoration.md) |
| ARCH-265 | candidate | behavior-defect | [ARCH-265 — AI backend scenario cleanup failures must affect their results](ARCH-265-weighted-ai-cleanup-outcome.md) |
| ARCH-266 | candidate | behavior-defect | [ARCH-266 — Dynamic status assertions must select their Controller and attachment](ARCH-266-body-dynamic-controller-status-projection.md) |
| ARCH-272 | confirmed | behavior-defect | [Bind EdgionTls aggregate children to their own complete listener contexts](ARCH-272-edgion-tls-aggregate-child-context.md) |
| ARCH-280 | confirmed | behavior-defect | [Bind metric assertions to the traffic produced by the current case and phase](ARCH-280-lb-case-metric-window.md) |
| ARCH-281 | candidate | behavior-defect | [Verify the LeastConn retry storm actually contains the configured retry attempts](ARCH-281-leastconn-retry-attempt-evidence.md) |
| ARCH-282 | candidate | behavior-defect | [Make positive cipher probes assert completed, fixture-permitted negotiation](ARCH-282-cipher-positive-negotiation-evidence.md) |
| ARCH-283 | candidate | behavior-defect | [Separate probe setup and transport failure from expected mTLS rejection](ARCH-283-mtls-negative-probe-outcomes.md) |
| ARCH-288 | candidate | behavior-defect | [Bound OpenSSL child lifetimes in the existing TLS scenario helpers](ARCH-288-tls-scenario-openssl-child-lifetime.md) |
| ARCH-296 | candidate | architecture-debt | [Consume the resolved AI listener port in lifecycle scenario requests](ARCH-296-ai-scenario-resolved-listener.md) |
| ARCH-304 | candidate | behavior-defect | [ARCH-304 — Complete Kubernetes execution ownership for the five GraphQL and transcoder dynamic scenarios](ARCH-304-kubernetes-graphql-transcoder-dynamic-ownership.md) |
| ARCH-312 | candidate | behavior-defect | [Read complete fixed echoes before judging L4 connection health](ARCH-312-l4-echo-stream-framing.md) |
| ARCH-336 | candidate | behavior-defect | [Wait for both prepared policies before asserting oldest-wins](ARCH-336-prepared-conflict-barrier.md) |
| ARCH-337 | candidate | behavior-defect | [Preserve metric observation failures before computing policy deltas](ARCH-337-metric-observation-errors.md) |
| ARCH-338 | candidate | behavior-defect | [Require a Gateway no-route outcome in the kind-denial probe](ARCH-338-kind-denial-protocol-outcome.md) |
| ARCH-339 | candidate | behavior-defect | [Require observed Listener no-conflict verdicts](ARCH-339-observed-listener-no-conflict.md) |
| ARCH-340 | confirmed | behavior-defect | [Require an observed HTTP rejection in the excessive-XFF probe](ARCH-340-xff-rejection-response.md) |
| ARCH-341 | candidate | behavior-defect | [Separate unavailable TCP listeners from ConnectionFilter denial](ARCH-341-tcp-filter-establishment-evidence.md) |
| ARCH-342 | candidate | behavior-defect | [Assert the header markers already sent by the BackendTLS echo cases](ARCH-342-backend-tls-header-markers.md) |
| ARCH-343 | candidate | behavior-defect | [Distinguish delegation rejection from an unknown backend RPC](ARCH-343-grpc-delegation-rejection-attribution.md) |
| ARCH-344 | candidate | behavior-defect | [ARCH-344 — Supply the retry-quota fixtures required by the selected Kubernetes SingleProvider suite](ARCH-344-k8s-selected-retry-quota-fixtures.md) |
| ARCH-345 | resolved | behavior-defect | [ARCH-345 — Configure the response-body probe required by the Kubernetes BodyPlugins suite](ARCH-345-kubernetes-wasm-response-probe-mode.md) |
| ARCH-346 | candidate | behavior-defect | [Establish the supported WAF probe's loopback identity in the Kubernetes topology](ARCH-346-kubernetes-supported-waf-loopback-probe.md) |
| ARCH-347 | confirmed | behavior-defect | [Require endpoint evidence in the AllEndpointStatus path-override case](ARCH-347-all-endpoint-status-path-override-evidence.md) |
| ARCH-348 | confirmed | behavior-defect | [ARCH-348: Negative response-body assertions must fail on unreadable observations](ARCH-348-canary-anti-spoof-unreadable-echo.md) |
| ARCH-349 | confirmed | behavior-defect | [ARCH-349: CSRF cookie-preservation failures publish the issued token](ARCH-349-csrf-cookie-failure-token-diagnostic.md) |
| ARCH-352 | candidate | behavior-defect | [Attribute the backend SAN rejection to its canonical request diagnostic](ARCH-352-backend-san-rejection-attribution.md) |
| ARCH-353 | confirmed | behavior-defect | [Require observed empty plugin logs instead of accepting probe failures](ARCH-353-empty-plugin-log-probe-errors.md) |
| ARCH-354 | confirmed | behavior-defect | [Attribute external-loopback rejection to the current request diagnostic](ARCH-354-external-loopback-diagnostic-attribution.md) |
| ARCH-355 | confirmed | behavior-defect | [The managed-overlay suite treats structural acceptance as proof that the overlay reference resolved](ARCH-355-managed-overlay-reference-resolution-evidence.md) |
| ARCH-373 | confirmed | behavior-defect | [Make the LDAP TLS Kubernetes mTLS case reach a certificate-required server](ARCH-373-ldap-kubernetes-required-mtls-fixture.md) |
| ARCH-377 | confirmed | behavior-defect | [Require the KeyAuth anonymous cases to observe their distinct identities](ARCH-377-key-auth-anonymous-identity-observation.md) |
| ARCH-381 | confirmed | architecture-debt | [Construct an encoded, invalid signature for the OIDC signature-rejection probe](ARCH-381-oidc-invalid-signature-fixture-encoding.md) |
| ARCH-382 | confirmed | behavior-defect | [Require the complete upstream path in PathNormalization forwarding assertions](ARCH-382-path-forwarding-complete-output-assertions.md) |
| ARCH-383 | confirmed | behavior-defect | [Correlate AllConditions assertions with the plugin entry being tested](ARCH-383-condition-target-log-correlation.md) |
| ARCH-385 | confirmed | behavior-defect | [Require an observed Admin rejection for malformed ConditionSet probes](ARCH-385-condition-set-observed-admin-rejection.md) |
| ARCH-386 | confirmed | behavior-defect | [Register the existing ProxyRewrite combined host and header assertion](ARCH-386-proxy-rewrite-combined-case-registration.md) |
| ARCH-389 | confirmed | behavior-defect | [Preserve the selected topology's Redis reference during quota reordering](ARCH-389-quota-reordering-topology-redis-reference.md) |
| ARCH-390 | confirmed | behavior-defect | [Distinguish the final RegionRoute jump from the preceding failover message](ARCH-390-region-route-failover-jump-observation.md) |
| ARCH-393 | confirmed | behavior-defect | [Select the actual mirrored capture before asserting its POST body](ARCH-393-request-mirror-capture-identity.md) |
| ARCH-394 | confirmed | behavior-defect | [Observe the query before claiming RequestMirror preserves it](ARCH-394-request-mirror-query-observation.md) |
| ARCH-401 | confirmed | behavior-defect | [Require fresh Wasm tick progress in the observation window](ARCH-401-wasm-tick-counter-window.md) |

## Current limitations

Pending files remain in scope. Recommendations await independent verification unless
explicitly marked confirmed. No builds, tests, implementation or commits are part of this run.
