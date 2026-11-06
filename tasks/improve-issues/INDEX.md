# Improvement review batch — INDEX

Global design review of all production (non-test) code, run 2026-11-05 on branch `feature-08-30`
(fce1aa529) and fully re-verified on 2026-11-06 against branch `feature-10-28`. Each finding compares Edgion with
Envoy / Istio / Kong / Traefik / APISIX / NGINX / ModSecurity where relevant and is classified as
**over-design**, **design-error** or **best-practice**. Low-value items (style, micro-optimizations,
accepted designs without new evidence, items already tracked under `tasks/`) were dropped by the reviewers.

This file is batch metadata, not a finding. One file per finding; delete a finding file when it is closed.

## Process

1. **Wave 1** — 12 feature-area reviewers (table below) plus the parent's cross-cutting `arch` analysis.
2. **Wave 2** — gap reviewers for units with incomplete first-pass coverage (`gw-plugin2`, `gw-infra2`, `ctrl-proc2`).
3. **Wave 3/4** — coverage closure: every file previously only skimmed was read in full
   (`gw-core3`, `gw-plug3`, `gw-ext3`, `gw-ai3`, `ctrl3`, `res3`, `res4`, `waf3`, `waf4`, `infra3`).
4. **Independent verification** — every finding was re-checked against the code by a separate skeptical
   verifier (or by the parent for `gw-infra-01`, `gw-core3-01`, `gw-plug3-02`, `waf4-01`). Each file ends with a
   `## Verification (independent)` section; 28 findings were downgraded, `ctrl-svc-06` was merged into `arch-01`,
   none were rejected.
5. **Re-verification on `feature-10-28`** — all 138 findings re-checked against the new branch (each file ends
   with `## Re-verification (feature-10-28)`): 0 fixed, 2 partially fixed (`arch-03` dependency hygiene done,
   `arch-05` guard now green — remaining scope rewritten), 136 still valid; stale line references updated.
6. **New code on `feature-10-28`** — the newly added production files (Redis backend/transport, downstream TLS
   policy, `kube_client.rs`) were reviewed and yielded `new-01..04`, verified by the parent.

## Severity

- **P0** correctness/security, fix now · **P1** significant · **P2** worth doing · **P3** direction / nice to have
- Totals (on `feature-10-28`): 2 × P0, 37 × P1, 67 × P2, 36 × P3 = **142 findings**

## Feature-area decomposition

| Prefixes | Feature area | Code |
|---|---|---|
| `arch` | Cross-cutting architecture (control/data-plane trust, overload, packaging, FIPS, governance) | whole workspace |
| `ctrl-proc`, `ctrl-proc2`, `ctrl3` | Controller resource processing, handlers, status, credentials; config centers residuals | `edgion-controller/src/conf_mgr/**` |
| `ctrl-center` | Config centers (K8s / FS / etcd), HA, ConfigSync server, federation | `conf_center/`, `conf_sync/`, `fed_sync/` |
| `ctrl-svc` | ACME, conf_sync CA, credential manager, DNS resolver, Admin API, authz | `edgion-controller/src/{services,api,authz,...}` |
| `gw-core`, `gw-core3` | Listeners, routing, proxy phases, timeouts, L4, PROXY protocol, forwarded headers, lifecycle | `edgion-gateway/src/{routes,runtime,conf_sync,...}`, `ctx.rs` |
| `gw-plugrt`, `gw-plug3` | Plugin framework, conditions, publication, stream plugins, outbound client | `plugins/{runtime,common,stream,publication}`, `plugins/http/common` |
| `gw-plugin`, `gw-plugin2` | HTTP plugins (auth, rate limit, CORS/CSRF, mirror, GraphQL, gRPC transcoding, steering) | `plugins/http/*` |
| `gw-ext`, `gw-ext3` | Proxy-Wasm, DSL, ext_proc | `plugins/http/{wasm,dsl,ext_proc}`, `edgion-dsl`, `edgion-wasm` |
| `gw-ai`, `gw-ai3` | AI gateway: routing, quota, accounting, guardrails, SSE, request body | `edgion-gateway/src/{ai,sse,request_body}` |
| `gw-infra`, `gw-infra2`, `infra3` | Backends, discovery, LB, health/outlier, TLS, LinkSys, observability, cache, GeoIP | `edgion-gateway/src/{backends,lb,tls,link_sys,observe,...}` |
| `res`, `res3`, `res4` | Resource/CRD API design, validation, reference authorization, path normalization | `edgion-resources` |
| `common` (+ `infra3`) | Egress/SSRF, DNS, outbound client, matchers/caches, CLI | `edgion-common`, `edgion-algo`, `edgion-cli` |
| `new` | Production code added on `feature-10-28` (Redis backend/transport, EGC downstream TLS policy, kube client) | `link_sys/providers/redis/{backend,transport}.rs`, `tls/{downstream_policy,curve_policy}.rs`, `resources/downstream_tls.rs`, `edgion-common/src/kube_client.rs` |
| `waf`, `waf3`, `waf4` | Native SecLang/CRS WAF engine, libinjection port, gateway glue | `edgion-waf`, `plugins/http/waf` |

## Suggested starting order

1. **P0**: `gw-infra-01` (mTLS bypass via TLS session resumption), `gw-core3-01` (one Gateway's invalid PROXY
   state kills every replica and crash-loops).
2. **Security P1s**: `res-01`, `res-02`, `res3-01`, `res4-01`, `res4-04`, `gw-core-02` (+ `ctrl-proc2-02`),
   `gw-core3-04`, `new-01`, `waf-01`, `waf3-01`, `waf4-01`, `gw-plugin-01`, `gw-infra2-01`, `gw-ai3-01`.
3. **Availability P1s**: `ctrl-center-02`, `ctrl-proc2-01`, `ctrl-svc-02`, `gw-core-01`, `gw-core-03`,
   `gw-core3-03`, `gw-ext-01/02/06/09`, `gw-plugin2-01`, `gw-plug3-01/02`, `gw-infra-02/04`, `waf-02`.
4. **Large-direction themes** (design first, then split):
   - Control-plane trust & scoping: `arch-01`, `ctrl-center-03`, `res-08`, `res4-04`.
   - Config churn amplification: `ctrl-proc-01..03`, `ctrl3-01`, `ctrl-center-04`, `gw-infra-02/03`,
     `gw-plugrt-01/02`, `gw-ext-09`, `waf-06`.
   - Availability under load: `arch-02`, `gw-core-01`, `gw-ext-01/02`, `gw-plugin2-01`, `waf-02`, `gw-core3-05`.
   - AI accounting accuracy: `gw-ai-01`, `gw-ai3-02/03/05`.
   - API surface consolidation: `res-04..07`, `gw-plugin-05/06`, `gw-plugin2-04`, `ctrl-center-05/08`.

## Review-knowledge pages found stale (fix alongside the code)

- `skills/04-review/concurrency/intentional-design/block-in-place-multi-thread-runtime.md` (premises false — `gw-ext-04`)
- `skills/08-kubernetes/01-gateway-api/01-not-implemented.md` (describes a GRPCRoute→HTTPRoute fallback that does not exist — `gw-core-03`)
- `skills/04-review/header-cert-auth/review-evidence/chain-verification-header-mode.md` (Kong `secure_source` — `gw-plugin-02`)
- `skills/04-review/link-sys/intentional-design/access-log-fallback-backfill-tradeoffs.md` ("never data loss" — `gw-infra2-02`)
- `skills/04-review/uri-normalization/mandatory-design/normalization-contract.md:225` (`%5C` claimed covered — `res4-01`)
- 17 pages still citing fork commit `0b0d8bba` (4 also name branch `edgion_0.9.0_v1`) — the guard itself is fixed on `feature-10-28` (`arch-05`)

## Findings (sorted by severity)

| ID | Severity | Category | Status on feature-10-28 | Title |
|---|---|---|---|---|
| gw-core3-01 | P0 | design-error | still valid | [One Gateway's inbound PROXY state (invalid, conflicting, or edited) kills every Gateway replica and crash-loops startup](gw-core3-01-proxy-protocol-policy-fatal-guard-fleet-crash-loop.md) |
| gw-infra-01 | P0 | design-error | still valid | [TLS session resumption crosses SNI / mTLS policy boundaries (client-auth bypass)](gw-infra-01-tls-session-resumption-bypasses-per-sni-mtls.md) |
| arch-01 | P1 | design-error | still valid | [conf_sync broadcasts every resource (including resolved secrets) to every gateway process under one shared identity](arch-01-conf-sync-broadcasts-all-secrets-to-every-gateway.md) |
| common-02 | P1 | design-error | still valid | [Shared DNS layer is cacheless, rebuilds a hickory resolver per lookup, ignores record TTLs; AI upstreams resolve on every request](common-02-dns-layer-cacheless-resolver-per-lookup.md) |
| ctrl-center-01 | P1 | design-error | still valid | [Cluster-scope rate-limit divisor (`gateway_instance_count`) is per-controller-replica and controller-wide, not per Gateway fleet](ctrl-center-01-gateway-instance-count-wrong-divisor.md) |
| ctrl-center-02 | P1 | design-error | still valid | [Hard-coded 15 s init-phase timeout overrides `cache_ready_deadline_secs` and can crash-loop the controller on large clusters](ctrl-center-02-hardcoded-init-phase-timeout-crashloop.md) |
| ctrl-proc2-01 | P1 | design-error | still valid | [Listener conflict detection ignores protocol, so TCP+UDP on one port are both dropped and HTTP/HTTPS/TLS/TCP mixes on one port crash Gateway startup](ctrl-proc2-01-listener-distinctness-ignores-protocol.md) |
| ctrl-proc-01 | P1 | design-error | still valid | [EndpointSlice churn cascades into Route, Gateway, EdgionTls and policy republish storms](ctrl-proc-01-endpoint-churn-republish-storm.md) |
| ctrl-svc-02 | P1 | design-error | still valid | [ACME renewal permanently stops after ~7 hours of failures and lets the certificate expire](ctrl-svc-02-acme-renewal-permanent-giveup.md) |
| gw-ai3-01 | P1 | design-error | still valid | [Guardrail treats "not a guardable SSE response" as a dependency failure — `stream:false` bypasses it under failOpen, and every non-streaming or provider-error response becomes an empty 503 under failClose](gw-ai3-01-guardrail-non-sse-responses-bypass-or-503.md) |
| gw-ai3-02 | P1 | design-error | still valid | [Output-token estimation counts every character of the JSON/SSE envelope, overcharging estimated usage by ~30-60x](gw-ai3-02-output-token-estimate-counts-json-envelope.md) |
| gw-ai-01 | P1 | design-error | still valid | [Definitive provider error responses are charged as token consumption (or retain full credential reservations)](gw-ai-01-error-attempts-charged-as-consumption.md) |
| gw-core3-03 | P1 | design-error | still valid | [An HTTP listener with HTTP→HTTPS redirect can never serve built-in ACME HTTP-01 challenges](gw-core3-03-https-redirect-listener-breaks-acme-http01.md) |
| gw-core3-04 | P1 | best-practice | still valid | [The proxy never sets X-Forwarded-Proto/Host/Port and forwards client-supplied values verbatim](gw-core3-04-forwarded-proto-host-never-set-client-values-passed-through.md) |
| gw-core-01 | P1 | design-error | still valid | [SIGTERM stops accepting immediately while `/ready` stays 200, and the drain outlives the default pod grace period](gw-core-01-shutdown-stops-accepting-while-ready-and-outlives-pod-grace.md) |
| gw-core-02 | P1 | design-error | still valid | [Missing Gateway-level and EGC-global HTTP plugin references are silently dropped (fail-open)](gw-core-02-missing-gateway-and-global-plugins-fail-open.md) |
| gw-core-03 | P1 | design-error | still valid | [Any GRPCRoute on a Listener makes native gRPC to every other hostname on that Listener return UNIMPLEMENTED](gw-core-03-listener-wide-grpcroute-presence-blocks-httproute-grpc.md) |
| gw-ext-01 | P1 | design-error | still valid | [Wasm pins one exclusive VM per in-flight request, capping route concurrency at 64](gw-ext-01-wasm-exclusive-vm-per-request-concurrency-ceiling.md) |
| gw-ext-02-ext-proc-hardcoded-64 | P1 | design-error | still valid | [ExtProc caps each target at 64 concurrent streams and uses one unbalanced HTTP/2 connection](gw-ext-02-ext-proc-hardcoded-64-stream-cap-single-connection.md) |
| gw-ext-06 | P1 | design-error | still valid | [ExtProc treats valid Envoy no-op verdicts (empty `HeadersResponse`/`BodyResponse`, `clear_route_cache`) as bad verdicts](gw-ext-06-ext-proc-rejects-valid-envoy-noop-verdicts.md) |
| gw-ext-09 | P1 | design-error | still valid | [Wasm configuration reload publishes an unloaded runtime, so traffic stalls on fetch/compile and a registry outage breaks working routes](gw-ext-09-wasm-reload-publishes-unwarmed-runtime.md) |
| gw-infra2-01 | P1 | design-error | still valid | [BackendTLSPolicy `subjectAltNames` semantics are inverted/additive: hostname is still authenticated, and wildcard SANs in the backend certificate never match](gw-infra2-01-backend-tls-san-validation-semantics-wrong.md) |
| gw-infra-02 | P1 | design-error | still valid | [Any EndpointSlice change resets health/outlier state for the whole Service and opens a 503 window](gw-infra-02-endpoint-churn-resets-health-and-outlier-state.md) |
| gw-infra-04 | P1 | design-error | still valid | [Passive outlier ejection is cancelled by any straggling successful response](gw-infra-04-outlier-ejection-cancelled-by-in-flight-success.md) |
| gw-plug3-01 | P1 | design-error | still valid | [Managed outbound health is one latching circuit for a whole endpoint pool; retries reuse the failed endpoint](gw-plug3-01-managed-outbound-health-is-one-latching-circuit-per-pool.md) |
| gw-plug3-02 | P1 | design-error | still valid | [Plugins read only the `Host` header and miss the HTTP/2 `:authority`, breaking redirects, OIDC login, host-scoped access rules, and ForwardAuth context](gw-plug3-02-plugins-read-host-header-and-miss-h2-authority.md) |
| gw-plugin2-01 | P1 | design-error | still valid | [gRPC-JSON transcoding allows 2 concurrent exchanges per process and GraphQL plugins allow 4 concurrent evaluations; excess requests get an immediate 503](gw-plugin2-01-transcoder-and-graphql-global-try-acquire-admission.md) |
| gw-plugin-01 | P1 | design-error | still valid | [ForwardAuth / ExternalAuth lets client-supplied identity headers through when the auth response omits them](gw-plugin-01-forward-auth-upstream-headers-not-stripped.md) |
| gw-plugin-03 | P1 | design-error | still valid | [OIDC session keeps the access token only in per-replica, per-generation memory](gw-plugin-03-oidc-access-token-only-in-replica-memory.md) |
| new-01 | P1 | design-error | new on 10-28 | [An unvalidated `EdgionGatewayConfig.downstreamTls.ciphers` value is admitted and then aborts every TLS handshake of the class](new-01-egc-downstream-cipher-typo-breaks-every-tls-handshake.md) |
| res3-01 | P1 | design-error | still valid | [EdgionAcme writes its certificate Secret into any namespace and overwrites any existing Secret there](res3-01-acme-certificate-secret-written-to-any-namespace.md) |
| res4-01 | P1 | design-error | still valid | [Path normalization decodes `%5C` into a literal backslash but never treats `\` as a separator, so `..\` traversal survives normalization while the security contract claims `%5C` is covered](res4-01-path-normalization-decodes-backslash-without-treating-it-as-a-separator.md) |
| res4-04 | P1 | design-error | still valid | [`namespace/name` LinkSys references (Redis, Webhook, DSL, AI quota, EdgionBackend) cross namespaces with no ReferenceGrant, letting any tenant drive another tenant's Redis and credentialed webhooks](res4-04-linksys-string-refs-cross-namespace-without-referencegrant.md) |
| res-01 | P1 | design-error | still valid | [Gateway `edgion.io/edgion-plugins` attaches another namespace's EdgionPlugins without ReferenceGrant](res-01-gateway-plugin-annotation-cross-namespace-without-referencegrant.md) |
| res-02 | P1 | design-error | still valid | [EdgionTls `hosts` are not constrained to the attached listener, so any attaching namespace can take over another hostname's certificate](res-02-edgiontls-hosts-not-bound-to-listener-hostname.md) |
| waf3-01 | P1 | design-error | still valid | [Numeric operators and setvar arithmetic abort the whole transaction on client-controlled non-i64 input; one CRS PL1 `Range` header turns every request into a 500](waf3-01-integer-operators-abort-transaction-on-client-input.md) |
| waf4-01 | P1 | design-error | still valid | [`@detectSQLi` whitelists 3-token `INTO OUTFILE` / `INTO DUMPFILE` payloads when the keyword is not upper case (divergence from libinjection)](waf4-01-sqli-into-outfile-whitelist-case-sensitive-false-negative.md) |
| waf-01 | P1 | design-error | still valid | [Body inspection cutoff (default `onLimit: continue`) skips every remaining WAF phase, including non-body rules and CRS blocking evaluation](waf-01-inspection-cutoff-skips-all-remaining-phases.md) |
| waf-02 | P1 | design-error | still valid | [No per-request CPU bound on WAF evaluation — 32 MiB default window, per-rule (not per-transaction) work budget, PikeVM-only regex, run inline on the Tokio worker](waf-02-no-per-request-cpu-bound-on-inspection.md) |
| arch-02 | P2 | best-practice | still valid | [No process-level overload protection (connection admission / memory-pressure shedding)](arch-02-no-process-level-overload-protection.md) |
| arch-04 | P2 | design-error | still valid | [FIPS build covers only downstream/Pingora-upstream TLS; conf_sync and all plugin egress TLS use non-FIPS `ring`](arch-04-fips-build-leaves-control-and-egress-tls-on-ring.md) |
| common-01 | P2 | design-error | still valid | [Egress classifier misses well-known cloud metadata addresses (Alibaba 100.100.100.200, AWS/GCP IPv6 IMDS)](common-01-egress-classifier-misses-cloud-metadata-addresses.md) |
| common-03 | P2 | design-error | still valid | [Egress SSRF guard is opt-in per call site and fragmented; IP literals bypass it and several paths diverge from the canonical policy](common-03-egress-guard-opt-in-and-fragmented.md) |
| common-04 | P2 | design-error | still valid | [`edgion-cli apply --dry-run` validates nothing although the Controller exposes `/api/v1/validate/{kind}`](common-04-cli-apply-dry-run-validates-nothing.md) |
| ctrl3-01 | P2 | design-error | still valid | [Controller re-runs every WAF profile/optional-bundle compile on each EdgionPlugins reconcile, inline on the single EdgionPlugins worker](ctrl3-01-controller-waf-trial-compiles-block-edgionplugins-worker.md) |
| ctrl3-02 | P2 | design-error | still valid | [FileSystem/etcd backends do not supervise kind workers, and a post-init backend error parks the Controller as a non-ready zombie](ctrl3-02-local-backends-unsupervised-workers-and-post-init-zombie.md) |
| ctrl-center-03 | P2 | best-practice | still valid | [Kubernetes ingest watches core kinds cluster-wide as full objects and syncs unreferenced Services/EndpointSlices/ConfigMaps to every gateway](ctrl-center-03-unscoped-core-kind-ingest.md) |
| ctrl-proc2-02 | P2 | design-error | still valid | [EdgionGatewayConfig reports `Accepted=True` while its outboundTls Secrets, global plugins and IP lists are missing](ctrl-proc2-02-egc-status-hides-unresolved-references.md) |
| ctrl-proc-02 | P2 | best-practice | still valid | [Dependency hooks re-fire on every reconcile, and loop safety relies on a TriggerChain that silently drops requeues](ctrl-proc-02-no-output-equality-gate-on-publish.md) |
| ctrl-proc-03 | P2 | design-error | still valid | [Gateway ownership replay requeues every object of nine kinds cluster-wide, once per Gateway](ctrl-proc-03-gateway-ownership-replay-requeues-whole-cluster.md) |
| ctrl-proc-04 | P2 | design-error | still valid | [Bounded workqueue channels make a worker's own requeue block, so a large kind can deadlock itself](ctrl-proc-04-workqueue-bounded-send-self-deadlock.md) |
| ctrl-proc-05 | P2 | over-design | still valid | [About 20 hand-maintained reverse indexes with ad-hoc string keys replace one generic dependency graph](ctrl-proc-05-hand-maintained-dependency-indexes.md) |
| ctrl-svc-01 | P2 | design-error | still valid | [Controller Admin API proxies unredacted cluster-wide Secret read/write with the controller ServiceAccount](ctrl-svc-01-admin-api-cluster-secret-proxy.md) |
| ctrl-svc-03 | P2 | best-practice | still valid | [ACME HTTP-01 activates challenges after a blind 5 s sleep instead of a readiness self-check](ctrl-svc-03-acme-http01-blind-propagation-wait.md) |
| ctrl-svc-05 | P2 | design-error | still valid | [Controller DNS resolver re-LISTs every Service cluster-wide every 15 s and resolves serially](ctrl-svc-05-dns-resolver-full-list-poll.md) |
| ctrl-svc-07 | P2 | design-error | still valid | [Controller-side OAuth2 token acquisition makes data-plane requests depend on control-plane uptime](ctrl-svc-07-credential-manager-control-plane-coupling.md) |
| gw-ai3-03 | P2 | design-error | still valid | [Native Messages usage observation validates `stop_reason` against a closed list — a new Anthropic stop reason kills every quota-Reject stream and drops accounting fleet-wide](gw-ai3-03-native-messages-closed-stop-reason-whitelist-breaks-accounting.md) |
| gw-ai3-04 | P2 | design-error | still valid | [Native Messages metadata inspection rejects valid long conversations (>4096 messages or content blocks) as "malformed body"](gw-ai3-04-messages-metadata-inspection-rejects-long-conversations.md) |
| gw-ai3-05 | P2 | design-error | still valid | [Adaptive-routing telemetry silently drops a process-phase-dependent share (0-100%) of every second's samples, and resets on any backend edit](gw-ai3-05-adaptive-telemetry-drops-phase-dependent-share-of-samples.md) |
| gw-ai3-06 | P2 | best-practice | still valid | [Guardrail re-decodes the entire semantic window after every SSE record — O(R²) JSON-string decoding per window on the streaming hot path](gw-ai3-06-guardrail-window-quadratic-semantic-rescan.md) |
| gw-ai3-07 | P2 | design-error | still valid | [Every AI request pins 2-3 full copies of its request body (plus a spill file) in the shared body budget for the entire streaming response](gw-ai3-07-ai-requests-pin-multiple-body-copies-for-stream-lifetime.md) |
| gw-ai3-08 | P2 | design-error | still valid | [Credential-first retry rounds spend the retry budget on credentials of a backend that is failing as a whole, so provider failover rarely happens](gw-ai3-08-credential-first-retry-defeats-backend-failover.md) |
| gw-ai-02 | P2 | design-error | still valid | [LowestLatency / LowestUsage are deterministic argmin over lagging telemetry (herding), diverging from the canonical P2C/Peak-EWMA balancer](gw-ai-02-adaptive-selection-argmin-herding.md) |
| gw-core3-02 | P2 | design-error | still valid | [L4 stream policies only see the direct peer, so behind PROXY protocol every per-IP limit, IP list and GeoIP rule applies to the load balancer](gw-core3-02-l4-stream-policy-blind-to-proxy-protocol-client-ip.md) |
| gw-core3-05 | P2 | design-error | still valid | [The UDP session table is exhaustible by spoofed sources, and each half-open session pins a socket, a task and a 64 KiB buffer for 60 s](gw-core3-05-udp-session-table-exhaustible-by-spoofed-sources.md) |
| gw-core-04 | P2 | design-error | still valid | [`timeouts.request` is not a wall-clock deadline once the upstream exchange starts](gw-core-04-route-request-timeout-not-a-wall-clock-deadline.md) |
| gw-core-05 | P2 | design-error | still valid | [Established L4 connections and UDP sessions are never re-validated after route, backend or policy changes](gw-core-05-l4-sessions-never-revalidated-after-config-change.md) |
| gw-ext3-01 | P2 | design-error | still valid | [DSL compiler recursion is not bounded by the parser nesting guard; one CR line uses ~1.4 MiB of a 2 MiB worker stack](gw-ext3-01-dsl-compiler-recursion-unbounded-by-parser-nesting-guard.md) |
| gw-ext3-03 | P2 | design-error | still valid | [Wasm preparation-capacity refusal is cached as a 30 s module-load failure, so a 5th concurrently loading plugin fails even when every source is healthy](gw-ext3-03-wasm-preparation-refusal-negative-cached-as-load-failure.md) |
| gw-ext3-04 | P2 | design-error | still valid | [Wasm OCI memory cache is keyed only by digest, so a namespace without registry credentials can load another namespace's private module](gw-ext3-04-wasm-oci-cache-bypasses-registry-authorization.md) |
| gw-ext3-05-wasm-callouts-executed-serially-behind-64 | P2 | best-practice | still valid | [Wasm `proxy_http_call` callouts run one at a time per request behind a 64-permit process-wide gate](gw-ext3-05-wasm-callouts-executed-serially-behind-64-global-permits.md) |
| gw-ext-03 | P2 | best-practice | still valid | [Wasm guest code and cold instantiation block Tokio worker threads for up to 10 s per hook](gw-ext-03-wasm-guest-runs-synchronously-on-tokio-workers.md) |
| gw-ext-04 | P2 | design-error | still valid | [DSL response stage blocks the worker per response (`block_in_place`) and panics with `work_stealing: false`](gw-ext-04-dsl-response-stage-block-in-place-bridge.md) |
| gw-ext-05 | P2 | over-design | still valid | [Standalone Wasm response stages duplicate attached `response:` hooks and break Proxy-Wasm stream context](gw-ext-05-redundant-standalone-wasm-response-stages.md) |
| gw-ext-07 | P2 | best-practice | still valid | [Proxy-Wasm guest-metric and log-level host calls trap, breaking common SDK filters at load or per request](gw-ext-07-wasm-guest-metrics-abi-traps.md) |
| gw-infra2-02 | P2 | design-error | still valid | [Access-log ES backfill starves live access logs for its whole duration and can lose already-"backfilled" lines; the in-gateway replay feature is the wrong layer](gw-infra2-02-access-log-backfill-starves-live-logs.md) |
| gw-infra2-03 | P2 | design-error | still valid | [OTLP log sink loses records invisibly — SDK queue overflow and export failures bypass Edgion's drop accounting, and its queue is a hard-coded 2048](gw-infra2-03-otlp-log-sink-silent-loss.md) |
| gw-infra-03 | P2 | design-error | still valid | [Service-discovery update cost scales with total cluster size per event (quadratic on relist)](gw-infra-03-discovery-update-cost-scales-with-cluster-not-delta.md) |
| gw-infra-05 | P2 | design-error | still valid | [Response cache is structurally limited to near-zero hit ratio (global flush on every config publish, 64-entry ceiling, worst-case slot charging, no stampede control)](gw-infra-05-response-cache-hit-ratio-structurally-capped.md) |
| gw-plug3-03 | P2 | design-error | still valid | [Redis multi-policy quotas charge long windows for requests that a short window rejects (one script call per policy, gateway clock)](gw-plug3-03-redis-multi-policy-quota-charges-denied-requests.md) |
| gw-plugin2-02 | P2 | design-error | still valid | [RequestMirror sends the shadow request at its filter position, so requests later rejected by auth / WAF / rate limit are still mirrored, carrying unauthenticated client identity headers](gw-plugin2-02-request-mirror-fires-before-auth-and-rejection.md) |
| gw-plugin2-03 | P2 | design-error | still valid | [RegionRoute forwards across regions with no hop/loop guard and trusts request-carried routing hints on every hop](gw-plugin2-03-region-route-no-forwarding-loop-guard.md) |
| gw-plugin-02 | P2 | design-error | still valid | [HeaderCertAuth Header mode accepts a public certificate from any peer (no trusted-source gate)](gw-plugin-02-header-cert-auth-no-trusted-source-gate.md) |
| gw-plugin-04 | P2 | design-error | still valid | [RateLimitLocal is a fixed window advertised as sliding; zero or sub-second intervals disable limiting or can panic](gw-plugin-04-rate-limit-local-fixed-window-and-sub-second-interval-hazards.md) |
| gw-plugin-05 | P2 | over-design | still valid | [Two independent CORS engines (EdgionPlugins `Cors` vs native Gateway API `CORS`) with divergent semantics](gw-plugin-05-two-cors-engines.md) |
| gw-plugin-07 | P2 | best-practice | still valid | [CSRF plugin rotates its token on every safe request and has no Origin / Fetch-Metadata defense](gw-plugin-07-csrf-token-rotation-and-no-origin-check.md) |
| gw-plugrt-01 | P2 | design-error | still valid | [Every publisher input event flushes the whole response cache and recomposes every scope](gw-plugrt-01-publisher-every-event-flushes-response-cache-and-recomposes-world.md) |
| gw-plugrt-02 | P2 | design-error | still valid | [Any edit to an EdgionPlugins resource resets all of its HTTP plugins' runtime state](gw-plugrt-02-http-plugin-state-reset-on-any-source-edit.md) |
| gw-plugrt-03 | P2 | over-design | still valid | [ExtensionRef re-implements every stage dispatch loop and has already diverged (no per-plugin latency for route-attached plugins)](gw-plugrt-03-extension-ref-duplicates-stage-dispatch-loops.md) |
| gw-plugrt-05 | P2 | best-practice | still valid | [L4 per-source-IP connection limiting exists only as a Redis call on the serial accept path; no local per-IP or concurrent-connection limit](gw-plugrt-05-connection-limits-lack-local-per-ip-and-concurrency.md) |
| infra3-01 | P2 | design-error | still valid | [EndpointSlice readiness treats unknown `ready` as not-ready and never falls back to serving-terminating endpoints](infra3-01-endpointslice-readiness-semantics-drop-unknown-and-terminating-endpoints.md) |
| infra3-02 | P2 | best-practice | still valid | [Round-robin restarts at the lowest-address endpoint on every selector rebuild, synchronizing all Gateway replicas](infra3-02-round-robin-resets-to-lowest-address-on-every-rebuild.md) |
| new-02 | P2 | design-error | new on 10-28 | [The Redis Cluster client fails commands on `ASK`/`TRYAGAIN`, so every resharding or scale-out causes rate-limit and quota errors](new-02-redis-cluster-ask-tryagain-fail-commands-during-resharding.md) |
| new-03 | P2 | design-error | new on 10-28 | [Sentinel connections always reuse the data-node credentials, so Sentinel mode cannot reach unauthenticated or separately authenticated Sentinels](new-03-redis-sentinel-credentials-coupled-to-data-node-auth.md) |
| res3-02 | P2 | design-error | still valid | [GeoIpLocation accepts unknown country/continent codes, so a mistyped deny rule silently never matches](res3-02-geoip-country-codes-not-validated-at-ingest.md) |
| res4-02 | P2 | design-error | still valid | [GrpcJsonTranscoder binds `google.api.http` path variables from the normalised path, violating the HttpRule escaping rules and silently truncating values at `;`](res4-02-grpc-json-transcoder-binds-variables-from-normalized-path.md) |
| res4-05 | P2 | design-error | still valid | [mTLS `allowedSans` RegularExpression entries are evaluated as unanchored substring searches](res4-05-mtls-allowed-san-regex-is-unanchored-substring-match.md) |
| res-03 | P2 | design-error | still valid | [EdgionGatewayConfig mixes per-GatewayClass fields with process-global fields aggregated over every EGC in the cluster](res-03-egc-per-class-resource-with-process-global-fields.md) |
| res-07 | P2 | best-practice | still valid | [No canonical authenticated principal — every auth plugin projects identity its own way, consumers key on request headers](res-07-no-canonical-authenticated-principal.md) |
| waf3-02 | P2 | design-error | still valid | [JSON duplicate keys discard every earlier value before inspection — payload in the first occurrence bypasses all ARGS rules](waf3-02-json-duplicate-keys-drop-earlier-values-uninspected.md) |
| waf3-03 | P2 | design-error | still valid | [XML text values are split at every comment, CDATA and PI boundary, so `SEL<!---->ECT` style payload splitting evades `XML:/*` rules](waf3-03-xml-text-split-at-comments-and-cdata-enables-payload-splitting.md) |
| waf3-04 | P2 | design-error | still valid | [`expensiveChecks` defaults to false and silently disables CRS PL1 rule 922110 (multipart charset evasion) in the "standard" profile](waf3-04-expensive-checks-default-off-disables-pl1-multipart-charset-rule.md) |
| waf4-02 | P2 | design-error | still valid | [`ruleSelection.include` has no dependency closure, so a tag/phase/ID include silently removes CRS initialization and blocking-evaluation units and the WAF stops blocking while Accepted](waf4-02-rule-selection-include-drops-crs-infrastructure-units-silently.md) |
| waf-03 | P2 | design-error | still valid | [`t:htmlEntityDecode` requires a terminating `;` and case-sensitive names, diverging from ModSecurity/Coraza and browsers (XSS evasion)](waf-03-html-entity-decode-requires-semicolon.md) |
| waf-05 | P2 | design-error | still valid | [Client-caused body parse errors and input-limit exhaustion return 500 in `On` mode, with no rule-visible error variable](waf-05-client-caused-waf-failures-return-500.md) |
| waf-06 | P2 | design-error | still valid | [Each EdgionPlugins ships ~1 MB of resolved CRS text and compiles its own WAF generation; nothing is shared by content digest](waf-06-compiled-generation-not-shared-across-plugins.md) |
| arch-03 | P3 | over-design | partially fixed | [Every subsystem is always compiled into the Gateway — no build-time extension selection](arch-03-no-build-time-extension-selection.md) |
| arch-05 | P3 | design-error | partially fixed | [Review knowledge pages still cite the pre-bump Pingora fork revision after the move to `edgion_v0.9.1`](arch-05-pingora-source-guard-red-after-fork-branch-bump.md) |
| common-05 | P3 | best-practice | still valid | [`edgion-cli` is CRUD-only — no sync-status, config-diff, describe or diagnostics commands that istioctl / deck operators rely on](common-05-cli-lacks-operational-diagnostics.md) |
| common-06 | P3 | best-practice | still valid | [`radix_ip` is an uncompressed one-node-per-bit trie — ~100x memory amplification and up to 128 dependent loads per IPv6 lookup](common-06-ip-radix-uncompressed-bit-trie.md) |
| ctrl3-03 | P3 | design-error | still valid | [Built-in federation Center policy grants write on every EdgionConfigData type, including PROXY-protocol trust and WAF policy](ctrl3-03-federation-default-policy-grants-security-configdata-writes.md) |
| ctrl3-04 | P3 | design-error | still valid | [OAuth2 credential source rejects RFC-conformant token responses without a numeric `expires_in`](ctrl3-04-oauth2-token-response-requires-numeric-expires-in.md) |
| ctrl-center-04 | P3 | best-practice | still valid | [Watch fan-out re-clones and re-serializes every event per gateway per kind across a 4-task pipeline](ctrl-center-04-watch-fanout-per-gateway-reserialization.md) |
| ctrl-center-05 | P3 | over-design | still valid | [Two Kubernetes HA modes; the default `leader-only` is a cold standby that needs pod-label routing machinery](ctrl-center-05-dual-k8s-ha-modes.md) |
| ctrl-center-06 | P3 | best-practice | still valid | [Kubernetes Lease expiry is judged by comparing the holder's `renewTime` with the local wall clock](ctrl-center-06-lease-expiry-uses-cross-node-wall-clock.md) |
| ctrl-center-07 | P3 | design-error | still valid | [`EventStore.last_version_by_key` retains every key ever deleted for the life of the serving epoch](ctrl-center-07-event-store-version-map-grows-unbounded.md) |
| ctrl-center-08 | P3 | over-design | still valid | [Three production config-center backends, each with its own HA, status and write path](ctrl-center-08-three-config-centers-scope.md) |
| ctrl-proc-06 | P3 | over-design | still valid | [The generic ResourceProcessor hard-codes per-kind behavior through `dyn Any` downcast lists and kind-string checks](ctrl-proc-06-type-erased-kind-lists-in-generic-processor.md) |
| ctrl-svc-04 | P3 | best-practice | still valid | [ACME renewal window is an absolute 30 days and ignores certificate lifetime and ARI](ctrl-svc-04-acme-renewal-window-not-lifetime-aware.md) |
| gw-ext3-02 | P3 | design-error | still valid | [DSL `json_get`/`json_select` accept duplicate JSON keys (last wins) while `json.*` rejects them — a parser differential inside one sandbox](gw-ext3-02-dsl-json-get-accepts-duplicate-keys-last-wins.md) |
| gw-ext3-06 | P3 | best-practice | still valid | [ExtProc header-mutation policy is hard-coded, differs per phase, and lacks Envoy `HeaderMutationRules` controls](gw-ext3-06-ext-proc-hardcoded-inconsistent-header-mutation-rules.md) |
| gw-ext3-07 | P3 | best-practice | still valid | [DSL execution has no wall-clock deadline; external calls are bounded only per call, not by the request deadline](gw-ext3-07-dsl-execution-has-no-wall-clock-deadline.md) |
| gw-ext3-08 | P3 | design-error | still valid | [Wasm request-body hook copies the whole body into an unbudgeted `Vec`, bypassing BodyStorage memory admission](gw-ext3-08-wasm-request-body-hook-copies-body-outside-memory-budget.md) |
| gw-ext-08 | P3 | over-design | still valid | [DSL accepts operator-supplied bytecode that silently overrides `source`, a vestige that adds an audit hazard and attack surface](gw-ext-08-dsl-dual-source-bytecode-input.md) |
| gw-infra-06 | P3 | over-design | still valid | [Gateway materializes Etcd LinkSys connections that no data-plane feature consumes](gw-infra-06-gateway-runs-etcd-linksys-with-no-data-plane-consumer.md) |
| gw-plug3-04 | P3 | best-practice | still valid | [GrpcJsonTranscoder replaces every backend gRPC error with a fixed body and drops grpc-message and status details](gw-plug3-04-grpc-json-transcoder-drops-backend-error-status.md) |
| gw-plugin2-04 | P3 | over-design | still valid | [Three overlapping L7 access-control plugins (IpRestriction, RequestRestriction, RequestAccessPolicy) with three different policy models](gw-plugin2-04-three-overlapping-l7-access-control-plugins.md) |
| gw-plugin-06 | P3 | over-design | still valid | [Five upstream-steering plugins duplicate one skeleton; "Canary" does no canary (weighted) release](gw-plugin-06-upstream-steering-plugin-sprawl.md) |
| gw-plugrt-04 | P3 | design-error | still valid | [Response-body-stage conditions are re-evaluated per chunk (probability flips mid-response) for a stage that serves one plugin](gw-plugrt-04-body-stage-conditions-evaluated-per-chunk.md) |
| infra3-03 | P3 | design-error | still valid | [`wellKnownCACertificates: System` resolves to two different trust stores in the same Gateway](infra3-03-system-ca-means-two-different-trust-stores.md) |
| new-04 | P3 | best-practice | new on 10-28 | [Redis TLS checks Sentinel- and Cluster-discovered nodes against their announced IP, with no server-name option](new-04-redis-tls-verifies-discovered-ip-with-no-server-name-option.md) |
| res3-03 | P3 | design-error | still valid | [Shared condition `regex` lists are joined with a bare `\|`, so one pattern's inline flags leak into the others](res3-03-condition-regex-list-joined-without-grouping.md) |
| res3-04 | P3 | best-practice | still valid | [OpenID Connect accepts `http://` discovery and never pins the scheme or issuer of discovered endpoints](res3-04-oidc-accepts-cleartext-discovery-and-unpinned-endpoints.md) |
| res3-05 | P3 | design-error | still valid | [ProxyRewrite accepts invalid static `host`, `uri` and header names/values; at runtime the rewrite is skipped and the request is proxied unchanged](res3-05-proxyrewrite-static-fields-unvalidated.md) |
| res3-06 | P3 | best-practice | still valid | [`authFailureDelayMs` is unbounded on all nine auth plugins and holds the connection, so it is a resource amplifier rather than a brute-force control](res3-06-auth-failure-delay-unbounded.md) |
| res4-03 | P3 | best-practice | still valid | [GraphQL cost model charges every unconfigured list field as a single item, so the budget is fail-open unless operators enumerate every list field](res4-03-graphql-cost-unconfigured-list-fields-charged-as-single-item.md) |
| res-04 | P3 | over-design | still valid | [Reserved/unwired fields are published as operator API (upstreamResponsePlugins, ConfigData `active`/`Misc`, LDAP `keepalive`)](res-04-reserved-unwired-public-api-surface.md) |
| res-05 | P3 | best-practice | still valid | [EdgionPlugins exposes proxy callback phases as separate arrays instead of letting each plugin own its phases](res-05-plugin-api-exposes-proxy-phases-instead-of-plugin-owned-phases.md) |
| res-06 | P3 | best-practice | still valid | [Route/Gateway-scoped extension knobs live in ~34 untyped annotations instead of a targetRef policy (GEP-713)](res-06-route-and-gateway-knobs-via-annotations-instead-of-policy-attachment.md) |
| res-08 | P3 | best-practice | still valid | [Resolved secret material is embedded as whole `k8s Secret` objects inside every consuming resource instead of SDS-style named secrets](res-08-resolved-secrets-embedded-as-whole-secret-objects-in-consumer-specs.md) |
| waf3-05 | P3 | over-design | still valid | [The WAF engine carries a reimplemented Aho-Corasick crate, a private slab allocator and several proof-based optimization layers whose complexity is not justified by measured gains](waf3-05-waf-engine-speculative-optimization-layers-and-reimplemented-primitives.md) |
| waf-04 | P3 | best-practice | still valid | [No rule-ID-scoped conditional exclusions (`ctl:ruleRemoveById` / `ruleRemoveTargetById`), forcing coarse tag-family exclusions for false positives](waf-04-no-rule-id-scoped-runtime-exclusions.md) |

## Coverage ledger

Scope: production code of every workspace member. Test code (`tests/`, `*_test.rs`, `#[cfg(test)]`,
benches, fixtures) and the `edgion-tests` harness were skipped by request. Generated data tables
(`edgion-waf/src/sqli/generated_data.rs`, `xss/blacklists.rs`, `metrics_catalog` `DEFINITIONS`, proto codegen)
were skipped as data. Every other non-test file was read in full by at least one reviewer; per-file ledgers
are in each reviewer's report.

| Unit | Status |
|---|---|
| `edgion-controller` conf_mgr/sync_runtime | complete (ctrl-proc → ctrl-proc2 → parent → ctrl3 full read) |
| `edgion-controller` conf_center / conf_sync / fed_sync | complete (ctrl-center → ctrl3 full read) |
| `edgion-controller` services / api / authz / config / observe / cli | complete (ctrl-svc → ctrl3 full read) |
| `edgion-gateway` routes / runtime / ctx / conf_sync / config / services / api / response / util / request_* | complete (gw-core → gw-core3 full read) |
| `edgion-gateway` plugins runtime / common / stream / publication / http common / store | complete (gw-plugrt → gw-plug3 full read) |
| `edgion-gateway` plugins/http (all plugins) | complete (gw-plugin → gw-plugin2 → gw-plug3) |
| `edgion-gateway` wasm / dsl / ext_proc, `edgion-dsl`, `edgion-wasm` | complete (gw-ext → gw-ext3 full read) |
| `edgion-gateway` ai / sse / request_body | complete (gw-ai → gw-ai3 full read) |
| `edgion-gateway` backends / lb / tls / link_sys / observe / response_cache / cache / geoip / cli | complete (gw-infra → gw-infra2 → infra3 full read) |
| `edgion-resources` | complete (res → res3 → res4 full read) |
| `edgion-common`, `edgion-algo`, `edgion-cli` | complete (common → infra3 full read) |
| `edgion-waf` | complete (waf → waf3 → waf4 full read; libinjection port differential-tested vs libinjection-go v0.3.2) |
| `edgion-tests` | skipped (test harness, out of scope by request) |
