---
name: design-rules
description: MANDATORY pre-coding checklist. The single graded index (A–Z) of Edgion's cross-cutting design rules — redaction, logging-by-binary, observability restraint, cross-cutting unification, reuse-first, failure policy, schema/transport, workqueue, time, Rust safety, English-only, Kubernetes/Gateway-API-first, config/secrets, security defaults & admission, annotations, HTTP/proxy correctness, change discipline, CI guards, SSA/conf_sync, resource wiring, hot-path performance, concurrency, panic isolation, testing discipline, schema strictness, DSL/plugin extension. Read this BEFORE writing or reviewing any code; drill into the linked detail docs only when a rule applies.
---

# Design Rules — read before coding

> This is the **brief, authoritative index** of the design rules every change must respect.
> It is deliberately short: one line per rule (🔴 MUST rules get a little more), plus a
> pointer to the **detailed canon**. Do not duplicate the detail here — when a rule bites,
> follow the `→` link. `skills/03-coding/` and `skills/01-architecture/00-common/` are the
> canonical detail layer; this file is the map.

## How to read a rule

| Tier | Meaning | On violation |
|------|---------|--------------|
| 🔴 **MUST** | Non-negotiable. Violation is a bug or a security/correctness defect. | Fix before merge. Reviewers block. |
| 🟡 **SHOULD** | Strong default. Deviate only with a documented reason. | Justify in the PR / a code comment. |
| 🟢 **PREFER** | Good-practice nudge. | Apply when it costs nothing. |

Each entry: **rule** — *consequence if violated* → `deep-dive pointer`.

## Map

| | | |
|---|---|---|
| **[A](#a-sensitive-data--log-redaction)** Redaction & sensitive data | **[J](#j-rust-coding--graceful-degradation)** Rust & graceful degrade | **[S](#s-conf_sync-incremental-update--hot-reload)** conf_sync incremental / hot reload |
| **[B](#b-logging-by-binary-data-plane-vs-control-plane)** Logging by binary | **[K](#k-written-content-is-english-only-everything-on-disk)** English-only on disk | **[T](#t-resource-dependency-wiring--processor-invariants)** Resource wiring invariants |
| **[C](#c-observability-restraint-metrics--access-log--tracing)** Observability restraint | **[L](#l-kubernetes-mode--gateway-api-first)** K8s & Gateway-API first | **[U](#u-gateway-hot-path-performance--bounded-memory)** Hot-path perf & memory |
| **[D](#d-cross-cutting-unification--one-canonical-implementation-per-mechanism)** Cross-cutting unification | **[M](#m-config-flags--secret-resolution)** Config, flags & secrets | **[V](#v-concurrency--shared-state)** Concurrency & shared state |
| **[E](#e-reuse-first-dont-re-invent-a-mechanism)** Reuse-first | **[N](#n-security-defaults--lenient-admission)** Security defaults & admission | **[W](#w-panic-isolation--trust-boundaries)** Panic isolation & trust |
| **[F](#f-failure-policy--error-replies)** Failure policy & replies | **[O](#o-edgionio-annotation-conventions)** Annotation conventions | **[X](#x-testing-discipline)** Testing discipline |
| **[G](#g-resource-schema--transport-invariants)** Schema ↔ transport | **[P](#p-http--proxy-correctness-recurring-bug-classes)** HTTP / proxy correctness | **[Y](#y-schema--validation-strictness)** Schema & validation strictness |
| **[H](#h-controller-workqueue-discipline-controller-conf_sync-only)** Workqueue discipline | **[Q](#q-change-discipline--review-precedent)** Change discipline & precedent | **[Z](#z-dsl--plugin-extension-boundaries)** DSL & plugin extension |
| **[I](#i-time--duration--units)** Time / duration / units | **[R](#r-enforced-ci-guards-hard-gates)** Enforced CI guards | |

---

## A. Sensitive data & log redaction

> Detail: `skills/03-coding/01-log-safety.md`. Sensitive = secret/token/key/cert/password/PII.

**There are 5 redaction mechanisms — pick by the surface you're on, not one blanket wrap:**

| # | Surface | Mechanism | When |
|---|---------|-----------|------|
| 1 | Admin API `/configclient` output | serde adapter `crate::redact::{redact_opt, redact_required}` (`edgion-resources/src/redact.rs`) | a resolved Secret body embedded in a resource struct |
| 2 | Generated CRD schema | `#[schemars(skip)]` | any Controller-filled / non-operator field (see §G) |
| 3 | Access-log header/ctx projection | `BLOCKED_HEADER_NAMES` + `ACCESS_LOG_BLOCKED_CTX_KEYS` (`.../plugin_configs/access_log_extern.rs`) | `AccessLogExtern` naming a header/ctx key |
| 4 | Plugin error entries | `PluginLog::push_err(&'static str)` | any plugin runtime error |
| 5 | Plugin dynamic PII/error | `push_err_sensitive` / `push_sensitive` / `push_fmt` + `sensitive(...)` (`runtime/log.rs`) | value only known at runtime |

- 🔴 **A resolved-secret field needs BOTH #1 and #2** — `redact_opt`/`redact_required` (admin-output mask) *and* `#[schemars(skip)]` (keep it out of the CRD). They cover independent surfaces; one alone still leaks the other. Note #1 is an admin-API-only thread-local guard — **conf_sync still ships the real value**, so keep that channel trusted. → `edgion-resources/src/resources/common/secret_slot.rs`.
- 🔴 **Plugin error logs use `push_err(&'static str)`, never `push(&format!(...err...))` / `err.to_string()`** — the `&'static str` signature is a compile-time leak blocker against `Display` impls (`jsonwebtoken`/`reqwest` errors carry tokens/URLs). Dynamic error → `push_err_sensitive`; dynamic PII → `push_sensitive`/`push_fmt(sensitive(...))`. → `edgion-gateway/src/plugins/runtime/log.rs`.
- 🔴 **`Authorization`/`Cookie`/`X-Api-Key` headers are categorically un-nameable in access-log projection** (in `BLOCKED_HEADER_NAMES`, no override); ctx keys holding claims/tokens are hard-blocked (`ACCESS_LOG_BLOCKED_CTX_KEYS`). A new ctx key carrying claims/tokens → add it to the blocklist. → `.../plugin_configs/access_log_extern.rs`.
- 🔴 **Never log a full resource `spec`, backend `IP:Port` list, or plugin config JSON at `info`** (leaks topology + secret refs) — downgrade to `debug`/`trace`; `ConfHandler` logs record metadata only (key name, counts, `rv`/`sv`). → `01-log-safety.md`.
- 🔴 **Source attribution (`group=`, and for JwtAuth `iss=`/`kid=`/`alg=`) is logged ONLY on the auth-success line, never on any `Deny`/failure branch** — otherwise a brute-force probe becomes a multi-tenant topology/enumeration oracle. → `skills/04-review/security/mandatory-design/iron-rules.md`, `skills/01-architecture/00-common/04-grouped-list-pattern.md` §4.
- 🔴 **Auth-failure reason text must not discriminate "identity unknown" from "identity exists, credential wrong"** — keep a single tag (`Deny bad credential`; LDAP relies on RFC 4511 rc=49 being uniform). *A discriminated tag is a username-enumeration oracle even with zero timing signal.* → `skills/04-review/ldap-auth/`, `basic-auth/`.
- 🟡 **Control-plane `resolve_credential` duplicate-identity warns carry only pointer fields** (`secret_key`, `group`, length) — never the credential value, not even hashed (the control-plane instance of redaction). → `skills/04-review/basic-auth/mandatory-design/dup-key-first-wins.md`.

## B. Logging by binary (data plane vs control plane)

> Detail: `skills/03-coding/01-log-safety.md`, `skills/03-coding/00-logging-and-tracing-ids.md`.

- 🔴 **`tracing::*` (including `debug!`) is FORBIDDEN on any gateway data-plane hot path** — every call chain reachable from an HTTP/TCP/TLS/UDP proxy callback or plugin dispatch. It causes lock contention, per-request×QPS log explosion, and leaks. Use instead: **access log** (`AccessLogEntry`), **`ctx.err_log`/`ctx.record_error`**, **`PluginLog`**, **`GatewayMetrics`**. Exceptions are a **closed list** (config-load, startup/shutdown, pre-panic `error!`, access-log-send-failure in `pg_logging.rs`, a few constructor-time loads) — anything else needs `// tracing-allow: <reason>` + review. Guard: `cicd/checks/check_gateway_no_tracing.py`. → `01-log-safety.md`.
- 🔴 **Control-plane (`edgion-controller`) wraps `process_resource`/`process_delete` in a `tracing::info_span!`** carrying `kind`/`name`/`ns`/`rv`/`sv`; inner logs don't repeat span fields and use structured `key=value` with a `component` field. Surface reconcile state via **K8s status conditions**, not only logs. → `02-tracing-and-logging.md`.
- 🔴 **Propagate the `key_name`/`rv`/`sv` ID triple end-to-end** — `key_name` = `namespace/name` (in every control log + access log); `rv` = K8s `resourceVersion` (control-plane only, never in the access log); `sv` = sync-version generated early in `process_resource()`, carried to the data plane and emitted in the access log (`MatchInfo.sv`) so a single `sv` correlates access log → gateway sync → controller. → `00-logging-and-tracing-ids.md`.

## C. Observability restraint (metrics / access log / tracing)

> Detail: `skills/03-coding/observability/`. Layering: request → access log (gateway); events → tracing (controller); aggregates → Prometheus.

- 🔴 **Gateway metrics go through the `GatewayMetrics` struct** (`edgion-gateway/src/observe/metrics/registry.rs`), never a bare `metrics::counter!()`/`histogram!()` in plugin/business code — distributed registration causes cardinality drift. → `observability/01-metrics.md`.
- 🔴 **No high-cardinality label** — never `path`/`user_id`/`ip`/`trace_id`/raw-query as a label value; bound label domains to compile-time enums (≤ tens). Guard: `cicd/checks/check_metrics_inventory.py` (`DANGEROUS_LABELS` + bidirectional inventory match). → `01-metrics.md`.
- 🔴 **No new `Histogram` without an explicit bucket + cardinality review** (the sanctioned few are exceptions, not precedent — recall the `MIRROR_DURATION_MS` +Inf incident); **every Gauge +1 has a matching -1** (prefer RAII/`Drop`). → `01-metrics.md`.
- 🟡 **Prefer NOT to add a metric** — controller side, pass the 3-gate test (second-level alert? core Grafana SLI? cross-process aggregation invisible in single-instance logs?); any "no" → use `tracing::warn/error` + a status condition. Controller metrics may use direct `counter!`/`gauge!` macros (opposite of the gateway convention, by design). Finer per-request detail belongs in access-log aggregation, not a new metric. → `01-metrics.md`.
- 🔴 **Access log (gateway): one entry reconstructs the whole request** — record only key checkpoints (final plugin decisions + exception paths), never intermediate steps. → `observability/00-access-log.md`.
- 🔴 **`request_info.status` is the Nginx/OpenResty access-log `$status` analogue, NOT mutable OpenResty `ngx.status`, an upstream attempt, or a diagnostic mapping** — finalize it exactly once in terminal logging from Pingora's final downstream response, otherwise an explicit `final_downstream_status` already selected by the response/terminal path, otherwise numeric `0` for Nginx `000` unknown. Never derive it from `EdgionErrorCode`, a serialized diagnostic code, or an upstream attempt. Diagnostic codes are stable strings with no numeric representation or implied HTTP prefix. Raw proxy failure codes are control inputs: e.g. a gRPC timeout maps raw 504 to downstream HTTP 200 + `grpc_status: 4`, never access-log HTTP 504. Interim 1xx are excluded except terminal 101. Once a final header commits, a body timeout/disconnect does not rewrite its status; a selected terminal 5xx whose error-page delivery fails remains that 5xx plus `ClientClosed`. Use 499 only when downstream closure itself terminates the request before header commit and before another final downstream status owns the outcome; a selected 200 header-write failure remains 200 + `ClientClosed`. Per-attempt upstream statuses stay in `backend_context.upstreams`; response-stage snapshots and downstream working status use separate non-serialized fields. HTTP and `grpc_status` are independent. The bounded `diagnostics` array is one append-only, cross-kind ordered history of intrinsic `{kind, code}` entries: information is normal noteworthy handling, while errors record encountered failure/reject/degradation signals. An error diagnostic is NOT itself an alert condition because a later retry may recover; alert policy is code-specific and may also depend on final status and aggregate rate. Do not deduplicate, sort, or reclassify entries at serialization time; emit `diagnostics_full: true` if the hard limit drops an excess tail entry. → `observability/00-access-log.md`.
- 🟡 **Condition match descriptors are composite-scope diagnostics** — suppress the
  match-only atomic descriptor when a logical decision scope has zero/one leaf;
  emit it only for multi-leaf scopes, while always retaining `cond_skip`, final
  decisions, named-group attribution, and errors. Count once at runtime
  construction, never per request. → `observability/00-access-log.md`.
- 🟡 **Keep `PluginLog` entries ~15–40 bytes** (shared 128-byte tier-1 buffer; overflow drops entries); add a new `AccessLogEntry` field only if it's frequently needed, request-level, and bounded (no full body / long token). → `00-access-log.md`.
- 🟡 **Control-plane tracing carries a `component` field and structured `key=value`** (not spliced format strings); error/warn logs carry locating context (`kind`/`ns`/`name`/`error=%e`). Follow the level discipline (`error`=affects requests, `warn`=recoverable, `info`=lifecycle). → `02-tracing-and-logging.md`.

## D. Cross-cutting unification — one canonical implementation per mechanism

> Full registry with verified consumer lists & exemptions:
> `skills/01-architecture/00-common/05-unification-points.md`. These are all 🔴 — a second
> implementation of any of these is a bug.

- 🔴 **Outbound HTTP clients are built via `build_outbound_client`** (`edgion-common/src/link_sys_core/http_client.rs`) — *a bare `reqwest::Client::builder()` outside tests is an SSRF/pooling regression* → registry §1.
- 🔴 **Outbound TLS (CA/mTLS/verify) resolves through `resolve_outbound_tls`** — including non-reqwest dialers (tonic/ldap3/fred/rskafka) — *ad-hoc TLS bypasses trust policy* → registry §2.
- 🔴 **New egress with a config- or request-influenced destination uses `EgressResolver`** (or at minimum `classify_ip`) at dial time, disables proxy-env pickup, denies redirects unless per-hop re-checked — *SSRF surface* → registry (egress).
- 🔴 **Operator-configured DNS goes through the shared `ResolverSpec`** (`edgion-common/src/dns/`) — *no second resolver library*.
- 🔴 **Redis access goes through the LinkSys `RedisLinkClient`**; rate-limit Lua extends the shared `redis_rate_limiter` core — *no bespoke Redis client*.
- 🔴 **Key extraction uses `KeyGet`; IP allow/deny uses `ip_radix_tree`; client real IP comes from `real_ip_extractor`** — *never re-implement these*.
- 🔴 **Templated config fields use the shared `${type:name}` engine**; security-sensitive fields (deadlines, fail-open switches, destinations) use the restricted allow-list that rejects client-controlled variables — *client-controlled template var = injection*.

## E. Reuse-first (don't re-invent a mechanism)

> Before adding a mechanism, check §D's registry and these patterns. Detail: the linked docs.

- 🟡 **Per-request conditional execution reuses the shared bounded `ConditionSet` engine**, not a bespoke `run_if`/`skip_if` or predicate enum. `PluginConditions.skip` is evaluated first, but `skip`/`run` each contains an explicit `allOf` or bounded `anyOf`; neither action field implies its Boolean operator. Static on/off belongs on entry-level `enable`. Condition key sources go through `KeyGet`. → `skills/01-architecture/00-common/05-unification-points.md` §15.
- 🟡 **A list needing named-group source attribution uses the `*_groups: Vec<{name, items}>` pattern** (atomic replacement of the flat field, first-wins on cross-group duplicates + `warn`), not a bespoke tagging scheme. → `skills/01-architecture/00-common/04-grouped-list-pattern.md`.
- 🟡 **A new templated config field reuses the shared `${type:name}` engine** — never hand-rolled interpolation. Classify every templated field into one of three buckets: *internal* (full variable set), *reflected-to-client* (full set minus `secretRef`, e.g. dye value), *security-sensitive* (restricted, next rule).
- 🔴 **Security-sensitive templated fields (deadline, fail-open switch, destination) MUST pass through the restricted allow-list that rejects client-controlled variables** (`reject_client_controlled_vars`, allows only `ctx`/`remoteIp`/`matchedHttpRoute`/`matchedGrpcRoute`) — else a header/query/cookie can pin a call deadline (DoS) or flip a fail-open switch (auth bypass). Enforced at admission (`Accepted=False`). → `edgion-resources/src/resources/common/external_http.rs`.

## F. Failure policy & error replies

> Detail: `skills/04-review/security/mandatory-design/fail-closed-error-body-no-internal-detail.md`. The fail-open/fail-close config vocabulary is frozen — see `skills/01-architecture/00-common/05-unification-points.md`.

- 🔴 **Plugins reject via `PluginRunningResult::ErrResponse { status, body }`**; `ErrTerminateRequest` is allowed only for an inherently dynamic response that the plugin has already written before returning — otherwise the runtime fail-safe emits a 502 + `EdgionErrorCode::PluginTerminateWithoutResponse` diagnostic (a bug marker, not a feature). → `edgion-resources/src/filters.rs`.
- 🔴 **Fail-close error bodies never leak internal detail** — no internal type names, dependency error text, upstream addresses, or traces in the client `body`; that detail goes to `PluginLog::push_err` / access-log diagnostics. Use `body: None` or a short generic message. (Client-actionable validation errors that are part of the documented contract are not a leak.) → fail-closed-error-body doc.
- 🔴 **Do NOT add a new fail-open/fail-close config shape.** Four grandfathered shapes exist and are frozen (`failure_mode_allow`, `fail_open`, `OnRedisFailure`, `DslErrorPolicy`); no convergence is planned — new runtime-dependency plugins follow the auth/non-auth guideline (see `skills/01-architecture/00-common/05-unification-points.md`). Auth-class failures are fixed fail-close, no knob (forward_auth's `allowDegradation` degrade switch was removed 2026-07-20).
- 🟡 **An operator-configured HTTP dependency (retry / timeout / success-predicate / header assembly) reuses `ExternalHttpConnection`**, not a parallel failure vocabulary. → `edgion-resources/src/resources/common/external_http.rs`.
- 🔴 **A misconfigured/unresolvable auth secret still replies with the plugin's normal generic status (401), never a 500** — a 500 for "secret not configured" leaks server-misconfiguration to an anonymous client; the only sanctioned deviation is the Admin API's wholly-unconfigured bearer token, which fail-closes at 503 (still not fail-open). → `skills/04-review/jwt-auth/`, `gateway-admin-auth/`.
- 🔴 **Cross-resource policy arbitration picks a single winner by `precedence_key` (oldest `creationTimestamp`, then name) and pushes that policy's section atomically** — never per-field merge across competing policies (`BackendTLSPolicy`, `EdgionBackendTrafficPolicy`), and the winner never partially clears the losing stores. *Per-field merge produces config no single YAML author wrote.* → `skills/01-architecture/02-gateway/07-backends.md`.
- 🔴 **A denied cross-namespace `allowRef` fails closed (deny-all); a denied `denyRef` fails under-block (drops only that deny set) — the asymmetry is intentional.** *Symmetrizing either way turns a missing `ReferenceGrant` into an outage, or silently disables a whitelist (auth bypass).* → `skills/04-review/`.
- 🟡 **Constant-time compare (`subtle::ConstantTimeEq`) applies only when an attacker-controlled value is equality-checked against a server secret with no independent blocking entropy** — `HashMap`-keyed lookups and wall-clock timing are out of scope ("constant-*time*" = comparison cost independent of data). → `skills/04-review/security/mandatory-design/iron-rules.md`, `auth-timing/`.
- 🟡 **Every HTTP auth plugin applies its pre-response brute-force delay via the shared `auth_common::apply_auth_failure_delay(delay_ms)`** — no plugin re-implements its own `sleep` (a §D-class unification). → `skills/04-review/auth-timing/`.
- 🟢 **Data-plane auth plugins stay industry-standard-minimal** — heavier/exotic verification (bcrypt, nonce-dedup replay store, response-HMAC, LDAP pooling) is delegated off the hot path to `forward_auth`/external authz, not embedded. → `skills/04-review/auth-timing/`.

## G. Resource schema ↔ transport invariants

> Detail + automated guards: `skills/01-architecture/05-resources/00-schema-transport-contract.md`
> and the CRD-coverage section in `AGENTS.md`.

- 🔴 **Every serialized resource field is classified**: operator-facing field → MUST appear in the CRD YAML (or the API server silently *prunes* it into a no-op); non-operator field (resolved host/port/listener, internal cache, resolved secret carried to the data plane) → MUST carry `#[schemars(skip)]` so it never enters the CRD schema — *pruned field = silent no-op; leaked internal field = schema/secret exposure*.
- 🔴 **A resolved secret value uses the `crate::redact::{redact_opt, redact_required}` serde adapter** (see §A) and its resolver-only struct does NOT derive `JsonSchema` — *defense-in-depth against schema-visible secret embedding*.
- 🔴 **Use `#[serde(skip)]` only for pure in-process caches the gateway re-derives** — never for a value the gateway needs over conf_sync.
- 🟡 **After adding/renaming a field, re-run the coverage guard** (`./cicd/checks/check_crd_coverage.sh`) and re-check this invariant → schema-transport-contract.md.

## H. Controller workqueue discipline (controller conf_sync only)

> Full rules + accepted exceptions: `AGENTS.md` §"Workqueue discipline" and
> `skills/01-architecture/01-controller/03-config-center/`.

- 🔴 **One workqueue + one worker per kind; dequeue is serial and must not wait.** Every runtime event (apply *and* delete) enqueues via `ResourceProcessor::on_event_key` — nothing calls `process_resource`/`process_delete` directly (except the pre-worker init phase). Reprocessing goes through **requeue**, never a direct call or a second queue — *out-of-order / dropped updates*.
- 🔴 **Handler processing contains no API calls, timers, sleeps, or other latency** — one slow item stalls every resource of that kind. (Accepted exception: inline status IO, by design.)
- 🔴 **The apply path fully replaces every side effect of the previous generation** (registries clear-before-add; dependents referenced only by the previous generation are requeued) — because the queue dedups by key, `on_delete` may never run for a recreated key.

## I. Time / duration / units

> Canonized in **`skills/03-coding/03-time-conventions.md`** (which surface → which shape,
> with the Gateway-API `metav1.Time` rationale). Four orthogonal concerns, four required
> shapes — not interchangeable. Single-helper discipline, not yet a lint (a grep guard is
> a candidate if the shape drifts again).

- 🟡 **Operator-facing durations (timeout/interval/ttl) use GEP-2257 strings via the shared `parse_duration()`** (`edgion-resources/src/duration.rs`; accepts `30s`/`5m`/`1h30m`/`500ms`, no bare numbers/decimals). Do NOT hand-roll a parser (RateLimit already had to be migrated back), and do NOT reach for `humantime` — it is not a workspace dependency. **CANON.**
- 🟢 **Monotonic elapsed time uses `Instant::now().saturating_duration_since(prior)`** — never `.elapsed()` / `.duration_since()`, which panic on non-monotonic clock skew (a panic on the data-plane hot path is a 5xx). **Enforced de-facto (audit closed).**
- 🟡 **Operator- / K8s-facing timestamps use `edgion_common::time::now_rfc3339()`** — RFC3339 UTC *seconds*, `Z`-suffixed (`2026-07-21T10:00:00Z`), matching how `metav1.Time` (the type behind Gateway API `Condition.LastTransitionTime`) serializes. One implementation; import it, never inline a bare `Utc::now().to_rfc3339()` (nanos + `+00:00`) for a persisted status field. Applies to status `lastTransitionTime`, ACME `lastRenewalTime`/`lastFailureTime`, the EndpointSlice `resolved_at` annotation. Reformatting an *existing* `metav1.Time` as an internal comparison key is out of scope. **CANON.**
- 🟢 **Hot-path wire-/access-log timestamps use epoch millis** (`chrono::Utc::now().timestamp_millis()`) — a deliberate, performance-motivated divergence from the RFC3339 status shape; do not "unify" it. `std::time::SystemTime` is for internal epoch math that never leaves the process — and never a bare `.unwrap()` on `duration_since(UNIX_EPOCH)`.

## J. Rust coding & graceful degradation

> Detail: `skills/03-coding/02-rust-coding-rules.md` (Rules 1–6 + checklist).

- 🔴 **No bare `unwrap()` in production code** — use `?` / `.ok_or_else(...)?` / `match`; a bare `unwrap()` is an unhandled panic (crash or 5xx). Exempt: `#[cfg(test)]`, and statically-valid literals (`Regex::new(r"literal")`). → Rule 2.
- 🔴 **`expect()` only when panic is logically impossible, and the message states *why*** (`expect("tail must exist when len > 0")`) — an unexplained message gives on-call zero signal. → Rule 2.
- 🔴 **Graceful-degrade over panic in long-running paths** (data-plane filters, TLS/stream proxy, control-plane reconcile loops): don't panic, fall back to a safe default (`None` / rule default / skip the bad entry), and leave a trail — data plane `plugin_log.push_err("<static kind>")`, control plane `tracing::warn!/error!` with context. `debug_assert!(false, ...)` is not the project idiom. Exceptions (fail-fast is correct): startup/one-shot init, exhaustive-match dead arms, data-corruption cases, tests. → Rule 5.
- 🔴 **Error messages are actionable** — what failed + context + remedy, not `format!("IO error: {}", e)`. → Rule 4.
- 🟡 **Import types via `use`; no inline path > 2 segments** (hoist `crate::a::b::Item`). Exempt: same-name disambiguation, macro-generated paths. → Rule 1.
- 🟡 **Clone judiciously — prefer references or `Arc`** — an `Arc` handle clone is cheap; cloning a big struct to read one field is the anti-pattern. → Rule 3.
- 🟡 **Prefer canonical helpers over local copies; preserve the workspace DAG** (`edgion-dsl → edgion-resources → edgion-common → {gateway, controller, cli}`; `edgion-resources` must never import `edgion-common`). Search the owning crate before writing a new parse/format/validate helper. → Rule 6.

## K. Written content is English-only (everything on disk)

> Full rule + anti-smuggling detail: `AGENTS.md` §"Coding constraints".

- 🔴 **Everything written to disk is English** — identifiers, comments, doc comments, TODO/FIXME, log/error string literals, commit messages, `skills/` and `docs/en/`. The only exception is Chinese user-facing docs under `docs/zh-CN/`.
- 🔴 **No smuggling past the English-only lint** — judge by runtime meaning, not source bytes. A value whose decoded form lands on a blocked codepoint is smuggling, whether spelled `\u{...}`, `\uXXXX`, percent-encoded UTF-8, runtime byte/hex/base64 construction, or `include_bytes!`. Use a lint-permitted equivalent that exercises the same code path (`hello%20world`, `foo%26bar`, `é`/`ñ`/`€`/`©`/`π`).

## L. Kubernetes-mode & Gateway API first

> Detail: `skills/08-kubernetes/01-gateway-api/`, `skills/08-kubernetes/04-api-version-upgrade.md`,
> `skills/01-architecture/01-controller/03-config-center/`.

- 🟡 **Design new features around K8s-mode Gateway API semantics first** — model behavior on the standard Gateway API resource/field before reaching for a proprietary `edgion.io/*` extension; an extension is the fallback when the standard has no equivalent, not the default. *Consequence: divergence from conformance and a surprise for operators who expect Gateway API semantics.*
- 🟡 **A feature (and its mechanisms — validation, status, requeue wiring) lives in the shared `ResourceProcessor`/`ConfHandler` layer, working the same across all config-center modes (Kubernetes / FS / etcd)** unless inherently backend-specific — only event source, status-persistence format, and HA/leader-election legitimately differ per `ConfCenter`. *Consequence: mode-dependent behavior drift.*
- 🔴 **API-version handling is atomic-upgrade-only — no probing, no fallback, no dual-watch.** Edgion is pre-release and does not support historical API versions: upgrade CRD bundle + deps + types + watchers + status writers + fixtures together and delete the old watcher/fallback/conversion code in the same change; keep watch/preflight/CRUD/status on one canonical version (a mixed-version path is a bug); FS/static inputs reject a non-canonical `apiVersion` before it enters the raw store. → `04-api-version-upgrade.md`.

## M. Config, flags & secret resolution

> Detail: `skills/02-features/02-config/`, `skills/02-features/01-binary-and-deployment/00-binaries.md`.

- 🔴 **A security tri-state config WARNs and falls through to the configured value on an unknown/typo token — never silently resolves to the unsafe state.** YAML booleans / legacy truthy-falsey tokens are rejected outright. *A config typo must never silently disable a security posture.* Example: `fips_policy` (`auto`/`on`/`off`), `EDGION_FIPS_POLICY`. → `AGENTS.md` §FIPS.
- 🔴 **Secret-bearing config never gets a CLI flag** — resolve via env-var-from-K8s-Secret → Secret-mounted config file only, to keep it out of `argv` (`ps`/`/proc`). *A `--admin-token`-style flag reintroduces a credential-leak vector.* → `03-auth-bootstrap.md`.
- 🔴 **A dangerous / test-only CLI flag requires a second env-var confirmation** (e.g. `--integration-testing-mode` is refused unless `EDGION_ALLOW_INTEGRATION_TESTING_MODE` is also truthy) — *a copy-pasted flag must not silently enable test surfaces in prod.* → `00-binaries.md`.
- 🟡 **Config source precedence is explicit per field, not assumed uniform** — document the actual order (e.g. `work_dir`: CLI>env>file; `fips_policy`: env>CLI>file; `admin_token`: env>file, no CLI). Don't copy a precedence template blindly. → `00-binaries.md`.

## N. Security defaults & lenient admission

> Detail: `skills/02-features/02-config/00-controller-config.md`, `03-auth-bootstrap.md`.
> Ties to the standing "operator owns the config" principle.

- 🔴 **Lenient admission is the DEFAULT model, not hard-reject.** A structurally valid-but-questionable resource is reported via `Accepted=False` / `PartiallyInvalid` status and still lands in the cache conf_sync serves; a downstream stage (gateway `compile_plugins`/`retain()`, or a runtime consumer falling back to a documented default + WARN) provides defense-in-depth. *Hard-rejecting instead removes the operator's signal and diverges from the two-part enforcement UX.* → `skills/04-review/review-method/`, `03-coding/observability/00-access-log.md`.
- 🟡 **Partial-good compilation is a distinct opt-in pair, not a global default** — offer both a strict all-or-nothing validator (`build_grouped_matcher`) and a partial-good one (`collect_entry_errors`/`build_grouped_matcher_partial`, skip bad entries + `PartiallyInvalid`) so a security-sensitive strict caller (`real_ip::trusted_ips`) is never silently made lenient. → `skills/01-architecture/00-common/04-grouped-list-pattern.md` §3.1.
- 🔴 **Bootstrap / transport security fails closed when unconfigured** — conf_sync mTLS omitted → non-zero exit (no plaintext fallback; `skip_tls: true` is the only loud, metric-tagged escape hatch); no admin token → 503 + startup WARN (never a panic). **Any new cross-process channel carrying resolved secrets follows this precedent** (refuse to start, don't silently downgrade to plaintext). → `03-auth-bootstrap.md`, `skills/01-architecture/03-controller-gateway-link/00-overview.md`.
- 🟡 **New opt-in hardening defaults to today's lenient behavior** so an upgrade never silently changes traffic (`plugin_policy` absent → allow-all; `path_normalization.*` default `false`). Operator override always wins. *Don't conflate this with bootstrap security above — the two default in opposite directions on purpose.* → `00-controller-config.md`.
- 🔴 **RBAC: a kind wildcard `["*"]` is forbidden with read verbs (`get`/`list`/`watch`)** — it exposes `Secret`/`EdgionTls` private-key material via blind proxy; only write-only verbs (`reload`) may wildcard. Kind matching is canonical PascalCase and case-sensitive (a wrong-case spelling silently never matches). → `00-controller-config.md`.
- 🟡 **Internal admin/control endpoints default to loopback bind (`127.0.0.1`)** (Envoy admin-interface model) — a wider interface is an explicit operator opt-in; a separate probe port carrying no sensitive data may bind `0.0.0.0`. → `edgion-gateway/src/cli/config.rs::DEFAULT_GATEWAY_ADMIN_LISTEN`.

## O. edgion.io/* annotation conventions

> Detail: `skills/02-features/05-annotations/00-annotations-overview.md`, `.../add-new-annotation.md`.

- 🔴 **System/controller-written keys and test-only keys are never hand-authored** and never documented as operator-facing config — controller/gateway-written (`hostname-resolution`, `sync-version`, `dns-resolved-at`) and test/tooling-only (`metrics-test-key`, `skip-load-validation`, `force-sync`). *A manifest setting these is a review red flag.* → `00-annotations-overview.md`.
- 🔴 **Removed annotation keys are deleted completely — no legacy alias/forward-port alongside the replacement** (the annotation-specific instance of the no-compat rule, §Q). → `00-annotations-overview.md`.
- 🟡 **Choosing the carrier (annotation vs `options` vs label) is a documented up-front decision** — record Carrier / scope / owner / parse-owner / value grammar / invalid-input behavior / hot-reload before coding, don't default to `metadata.annotations`. → `add-new-annotation.md`.
- 🟢 **Annotation value-parsing strictness is calibrated per-key risk** — don't force one blanket boolean/enum parser across all keys (`enable-http2` is lenient; `cert-provider` requires exact case-sensitive `"EdgionTls"`). → `add-new-annotation.md`.

## P. HTTP / proxy correctness (recurring bug classes)

> These are recurring review knowledge classes — recognize them in your own diff.
> Detail: `skills/04-review/SKILL.md`.

- 🔴 **Every `host:port` built from a dynamic IP must be IPv6-bracket-safe** — a bare `format!("{}:{}", ip, port)` is a presumptive bug (broke backend connect, DNS, redirects, PROXY-protocol, UDP binding). *Use the bracket-safe join.* → review knowledge `backends-F001`/`redirect-F001`/`net-F001`.
- 🔴 **A header "replace-all" op removes prior values by name first and preserves repeatable headers** (`Set-Cookie` = set-first-then-append), never collapses them via a single `insert_header`. *Dropped auth/cookie headers = security leak or broken login.* → review knowledge `F-001`/`runtime-03`.
- 🔴 **Un-map IPv4-mapped IPv6 literals (`::ffff:a.b.c.d`) and reclassify before loopback/link-local checks** — a V6-only branch lets `::ffff:127.0.0.1` / `::ffff:169.254.169.254` bypass the SSRF guard to loopback/cloud-metadata. Canonical: `classify_backend_addr` → `to_ipv4_mapped()`. → `edgion-gateway/src/util/net.rs`; `skills/04-review/ssrf/`.
- 🟡 **Route authorization is not proof of dial-address safety** — every upstream-dial path (dynamic upstream, RegionRoute, DirectEndpoint, mirror, LB) needs its OWN post-DNS-resolution loopback/link-local check near the final dial, independent of the route-authorization boundary (extends §D). → `skills/04-review/ssrf/mandatory-design/upstream-address-safety.md`.
- 🔴 **RFC-MUST host validations apply identically to HTTP and gRPC-over-h2 — no protocol carve-out** (empty/duplicate `Host`, `:authority`); the shared `NormalizedHostname` path is the single enforcement point. *An empty `:authority` landing on a catch-all GRPCRoute is a cross-tenant leak.* → `skills/04-review/host-header/`.
- 🟡 **Any `Location:` URL builder goes through the shared IPv6-bracket-preserving `host_without_port` helper and appends the request query unconditionally** — two sites shipped the same `split(':')`/dropped-`?query` bug before converging. → `edgion-gateway/src/util/net.rs::host_without_port`.
- 🔴 **A connection-reuse retry must not replay a buffered non-idempotent (POST/PATCH) body** — gate on `Method::is_idempotent()`, opt back in only via `edgion.io/retry-non-idempotent: "true"`. *Silent double-execution of a mutation on a keepalive reset.* → `skills/04-review/routing/regression-guards/retry-non-idempotent-method-guard.md`.
- 🟡 **Retry never fires before the configured minimum backoff** (GEP-1731 — when `remaining ≤ backoff`, fail fast with the deadline status); **1xx interim responses must not update final-response bookkeeping** (status/CB/retry/session), except 101 stays on the terminal path. → `skills/04-review/routing/`.
- 🔴 **Business code never reads `req_header().uri` directly — use `ctx.path()`/`request_path()` (or `session.get_path()`/`get_request_path()`).** Only 2 legitimate `set_uri()` write sites exist, and both immediately set `ctx.uri_finalized = true`; a third write desyncs `ctx.path()` from the real URI, so ACL/route/log silently read a stale path (the exact bypass URI normalization exists to close). → `skills/01-architecture/02-gateway/uri-normalization/SKILL.md`.
- 🔴 **SNI resolution, cert match, and `min_tls_version` are decided only in the early `select_certificate_callback`** (BoringSSL freezes the version range before `certificate_callback`) — new SNI/version policy lands there, not cert_cb; every handshake-reject logs exactly once (`log_ssl`), including aborts before cert_cb. → `skills/01-architecture/02-gateway/04-tls/SKILL.md`.
- 🔴 **CORS `allow_credentials: true` is rejected at admission with regex-based origin matching too** (`allow_origins_by_regex`), not just literal `*`/`**` — a regex origin is a de-facto wildcard; credentialed CORS needs an explicit `allow_origins` list. → `skills/04-review/cors/regression-guards/regex-origins-with-credentials-rejected.md`.

## Q. Change discipline & review precedent

> Detail: `AGENTS.md` §"Coding constraints", `skills/04-review/SKILL.md`, `skills/04-review/SKILL.md`.

- 🔴 **Pre-release: no backward-compat burden — but this does NOT license unrequested refactors.** Rename/move/delete freely with no shims / `#[deprecated]`-for-compat, but only within the requested scope; "keep changes minimal" governs everything else. *Both an unrequested compat shim and an over-broad refactor violate this.* → `AGENTS.md`.
- 🟡 **Scoped review decisions are precedent** — search the owning function in `skills/04-review/SKILL.md`, then verify the current rule, assumptions, and re-evaluation triggers before proposing a previously rejected fix. Superseded decisions are history, not current gates. → `skills/04-review/knowledge-routing.md`.
- 🟡 **RFC-divergence discipline: a MUST clause is fixed to comply; a SHOULD/MAY clause is evaluated against industry precedent (Envoy/nginx) and, if diverged, closed with a written decision** (clause + current code shape + why-we-diverge + re-evaluation trigger) — never silently ignored, never "fixed" against precedent without new evidence. → `skills/04-review/http1/`.

## R. Enforced CI guards (hard gates)

> These fail the build. Run them locally (`AGENTS.md` §"Pre-commit checks"); most run in parallel.

- 🔴 **SSA: every `Patch::Apply` uses forced `PatchParams` (`.force()`)** — an unforced apply 409-Conflicts forever once another field manager (POST-created object, `kubectl edit`, a pre-adoption Secret) owns the object (caused a real conf_sync CA-renewal outage). `Patch::Merge`/`Json`/`Strategic` are exempt. Guard: `cicd/checks/check_ssa_force.py`.
- 🔴 **Metrics inventory + label allowlist** — every emitted metric is listed in the inventory doc (bidirectional) and no off-allowlist high-cardinality label. Guard: `cicd/checks/check_metrics_inventory.py` (see §C).
- 🔴 **No `tracing::*` on the gateway data-plane hot path.** Guard: `cicd/checks/check_gateway_no_tracing.py` (see §B).
- 🔴 **CRD ↔ struct coverage** for the structural CRDs. Guard: `./cicd/checks/check_crd_coverage.sh` (see §G).
- 🔴 **Agent-doc consistency** — `skills/`/`AGENTS.md` cross-references, frontmatter, indexes must stay consistent. Guard: `cicd/checks/validate_agent_docs.py`.
- 🔴 **Supply-chain: `cargo deny` (license allowlist / banned-crate / source pinning via `deny.toml`)** + daily advisory-only cron; triage recorded as commented `ignore` entries, never silently bypassed. New/upgraded deps also run `cargo audit`. Don't run with `--all-features` (TLS/allocator features are mutually exclusive). → `skills/09-misc/cicd/02-github-workflow.md`.
- 🔴 **Bash under `set -e` never uses bare `((x++))`** — `((expr))` returns exit 1 when `expr==0`, silently killing the script on bash 5 (CI) while passing on bash 3.2 (macOS). Use `x=$((x + 1))`; route OS-specific behavior through `shell_compat.sh`. Guard: `cicd/checks/check_bash_set_e_increment.sh`.
- 🟡 **`fips` is an opt-in build-time Cargo feature** (implies `boringssl`; the canonical amd64/arm64 Boring 5 source path verifies the built module), NOT a runtime toggle and NOT part of the ordinary release workflow — never conflate it with the runtime `require_fips`/`fips_policy` knobs, never enable alongside `rustls`. → `skills/09-misc/SKILL.md`.

## S. conf_sync incremental update & hot reload

> Detail: `skills/01-architecture/02-gateway/11-conf-handler-guidelines.md`.

- 🔴 **No "fake incremental."** `ConfHandler<T>::partial_update(add, update, remove)` must not merge into the full dataset and then full-rebuild (that silently loses updates / hides O(n) cost behind a delta signature). If true incrementality isn't implemented yet, mark `// TODO: incremental rebuild` — don't emulate via `full_set`. → §3.1.
- 🟡 **Engine-shaped resources rebuild only the affected scope** (hostname/listener_key/service_key), reusing the rest via `Arc`; "full rebuild every partial_update" is acceptable only for low-churn Configuration-class resources (Gateway/GatewayClass/EdgionGatewayConfig). → §3.2.
- 🟢 **Route-table / engine swaps are atomic** (`ArcSwap::store` of a fully-built value) — never publish a half-built intermediate mid-rebuild. → §5.

## T. Resource dependency wiring & processor invariants

> Detail: `skills/01-architecture/01-controller/09-add-new-resource/00-guide.md`,
> `skills/01-architecture/01-controller/03-config-center/00-overview.md`.

- 🔴 **`on_change()` self-lookups use `RawStore`/`get_raw_any`, never `ServerCache`** — `on_change` runs *before* `save_to_cache`, so a `ServerCache` lookup of the resource being processed compiles but silently returns the stale prior generation. → add-new-resource guide Step 3.
- 🔴 **Any handler whose `parse()` calls `lookup_gateway()` registers in `gateway_route_index` via `on_change()`/`on_delete()`** — without it, a later Gateway/listener change never requeues the dependent and `resolved_listeners` freezes forever with no self-heal (EdgionTls shipped with this bug once). → `skills/08-kubernetes/01-gateway-api/SKILL.md`.
- 🟡 **Don't add per-kind exponential-backoff retry to a `ResourceProcessor`** — the design self-heals via a full cache rebuild on watcher reconnect; a bespoke retry loop duplicates that recovery path (`requeue_with_backoff` is intentionally unimplemented). → config-center overview.
- 🟡 **Treat the cross-resource requeue graph as cyclic by design** (Secret→resource, Service→route, route→Gateway-status all loop) — rely on the existing `TriggerChain` depth/cycle limits; a new dependency edge is a cycle-safety decision, not a reason to add an ad-hoc guard. → config-center overview.

## U. Gateway hot-path performance & bounded memory

> Detail: `skills/01-architecture/02-gateway/05-plugin-system.md`, `skills/04-review/cpu-memory/`.

- 🔴 **Build anything reused per-request ONCE at preparse/config-resolve, never lazily per-request** — regex-bearing `PluginConditions`/`Condition` compile in `EdgionPlugins::preparse()` (`compile_conditions`); `DslRuntime` is built once in `resolve_parts()`. *Per-request `Regex::new()` / engine rebuild explodes CPU under load.* → `05-plugin-system.md`.
- 🔴 **The per-chunk body phase (`UpstreamResponseBodyFilter`) allocates zero and takes no `PluginLog`** — use the compiled ConditionSet's log-less path, never `PluginLog`/`tracing`/`String` per chunk; aggregate once in `EdgionHttpContext`. *Long streams fire it thousands of times (leaks g2-001/g2-004).* → `traits/upstream_response_body_filter.rs`.
- 🔴 **Cache a caller-lifetime lookup on the owning long-lived struct at construction — never re-derive it per-request via a write-acquiring global map** (`DashMap::entry`, a `format!()`-built key); `entry()`/`or_insert_with` takes a shard *write* guard even on hits, serializing same-shard requests. → `04-review/cpu-memory/`.
- 🟡 **An established session/connection fast-paths around the full match/select/SSRF pipeline once pinned** (key off a session map) — re-running it per packet is discarded work. Avoid per-request heap allocation for transient bookkeeping (stack structure + fallback above a threshold).
- 🟡 **Enforce body/stream size caps WHILE streaming** (count received bytes, `stream_body_capped`), never buffer-the-whole-body-then-check — a `Content-Length` check is an optimization, not the load-bearing guard. Acquire a concurrency-limiting semaphore per unit-of-work, not per batch/cycle. → `04-review/cpu-memory/`.
- 🟡 **Default a plugin to the earliest sufficient phase (`RequestFilter`)** and pick its layer by data dependency — a Stream plugin sees only IP/listener-port/SNI/mTLS; anything needing header/cookie/query/path/ctx must be an HTTP `EdgionPlugin`. → `12-edgion-plugin-dev.md`.

## V. Concurrency & shared state

> Detail: `skills/04-review/concurrency/`, `skills/01-architecture/02-gateway/11-conf-handler-guidelines.md`.

- 🔴 **Never hold an async lock guard across a network round-trip** — clone the cheap handle (`Client`/`Channel`) under a brief guard, drop it, then do I/O. *A guard held across an RPC blocks every other reader for the RPC's duration.* → `04-review/concurrency/regression-guards/async-write-lock-across-rpc.md`.
- 🔴 **A check-then-mutate on shared state is ONE atomic operation** — in-memory: `compare_exchange` loop / `ArcSwap::rcu`; cross-process resource: a per-key async lock spanning the full get→modify→write. *A non-atomic RMW loses concurrent updates or over-admits past a limit.* → `04-review/concurrency/`.
- 🟡 **Jointly-invariant derived fields publish behind ONE `ArcSwap`/lock** (a single snapshot struct), never two back-to-back `store()` calls (a cross-epoch read window).
- 🟡 **`ArcSwap` is data-plane (gateway) only; the controller uses `RwLock`** — write pattern clone→modify→`store(Arc::new(...))`. Don't nest `ArcSwap` (a write-once inner swap inside an `ArcSwap`'d container pays an atomic-load per request for nothing). → `11-conf-handler-guidelines.md` §3.3.
- 🟢 **`std` `RwLock`/`Mutex` `.unwrap()` poisoning only matters if a thread can panic while holding the write lock** — an infallible critical section (`insert`/`clear`/assignment) doesn't need a fix (narrows §J's blanket unwrap-ban). → `04-review/concurrency/`.

## W. Panic isolation & trust boundaries

> Detail: `skills/01-architecture/02-gateway/dsl-engine.md`, `.../04-tls/`.

- 🔴 **Wrap any embedded execution engine (DSL VM, Wasm) or FFI boundary (BoringSSL callbacks) in `catch_unwind`** — a panic must not cross into Pingora's worker loop (fatal to the whole worker, not one request). Canonical: `vm::execute_safe`, `mtls_verify_callback`. → `dsl-engine.md` §5.
- 🔴 **The gateway independently re-validates a controller-precompiled artifact after deserialization** (DSL bytecode: `deserialize → Validator::validate → use`) — to catch hand-crafted bytecode declaring a lower limit than it uses; the controller compiler is not the sole security boundary. → `dsl-engine-security-model.md`.

## X. Testing discipline

> Detail: `skills/05-testing/`.

- 🔴 **Unit tests never hit network / real DB / real server** (that's integration) — use `tempfile`, never absolute paths; no empty "no panic" tests. → `00-unit-testing.md`.
- 🔴 **The untouchable test list is never deleted/merged for coverage reduction without stated replacement coverage** — data-structure invariants, error-message text, algorithm correctness, spec/wire-format compat, security boundaries (mTLS depth/SAN/cipher), bug-ID regressions, crypto/JWT. → `00-unit-testing.md`.
- 🔴 **Integration suites reuse the single shared Gateway+Controller topology** — no second `start_all`/kill-restart (CI fails on `pgrep -c ≥ 2`); a standalone topology needs a documented header exception. → `01-integration-testing.md`.
- 🔴 **Every new in-memory store wires into leak detection** (`count()`/`keys()` + `StoreStatsResponse` + an "empty reports zero" test) — else `cache_residue` can't see its leak. → `02-cache-residue-detection.md`.
- 🔴 **Never accept a snapshot refill (`UPDATE_EXPECT=1`) without reading the diff** — the refill is the test asking "did this change on purpose?". → `05-snapshot-testing.md`.
- 🟡 **A hot-reload-capable feature needs a dynamic-scenario integration test** driven by `wait_phase` (never a fixed `sleep`). → `01-integration-testing.md`.

## Y. Schema & validation strictness

> Detail: `skills/01-architecture/05-resources/`, `skills/04-review/review-method/`, `skills/04-review/deserialization/`.

- 🔴 **A new per-route knob on a standard Gateway API kind (HTTPRoute/GRPCRoute) is an `edgion.io/*` annotation parsed in `preparse()`, NEVER a new `spec` field** — the vendored structural CRD silently *prunes* an unknown field (a "feature" that never takes effect); the derived runtime field carries `#[serde(skip)] #[schemars(skip)]`. → `04-review/routing/`.
- 🔴 **`deny_unknown_fields` on credential/auth sub-structs (`SecretSlot`/`RedisAuth`/`EtcdAuth`/`WebhookAuth`) is a security boundary** — it makes an inline `password:`/`username:` fail to deserialize instead of silently running unauthenticated; don't relax it for forward-compat. → `edgion-resources/src/resources/common/secret_slot.rs`.
- 🟡 **A CRD subtree the API server does not prune (`x-kubernetes-preserve-unknown-fields`, e.g. plugin `config`) needs ingest-side unknown-key DETECTION, not just `deny_unknown_fields`** — serde is the only remaining gate, so a mistyped key is silently dropped with no admission error, and for a security plugin that degrades to a silent no-op / fail-open (`deny:` typo'd as `denys:` leaves an empty deny list, `Accepted: True`, nothing logged). Detection (warn, surface a status condition) is the ingest-side control for this shape; `deny_unknown_fields` remains the separate credential-injection defense on closed sub-structs — the two are not substitutes for each other. → `edgion-resources/src/schema_check/`, `skills/04-review/deserialization/SKILL.md`.
- 🟡 **Discriminator enums (plugin `type:`, resource kind) stay strict — no catch-all `Unknown` variant** — a typo hard-fails deserialization into a status condition, not a silent no-op. → `edgion_plugin.rs`.
- 🟡 **`PluginValidation::validation_error()` is stateless** (no I/O / mutation / cached `Option<String>`); runtime compilation lives in `compile()`/constructors. A disabled entry (`enable: false`) is inert — not compiled/validated, never flips `Accepted=False`. → `plugin_validation.rs`.
- 🟡 **Controller preparse/validate is advisory (writes `.status`); the gateway compile step (`compile_plugins`/`retain()`/`seal_fail_closed`) is the real drop point** — verify the gateway-side enforcement, not just the ingest status condition. → `plugin_store.rs`.
- 🔴 **A plugin's request-serving constructor (`::new`/`::create`/`::from_arc`) MUST call its own `validate_and_init()`** to compile derived matchers/regex/caches before the instance is cached for dispatch — skipping it silently fail-opens (an uncompiled ACL/CORS-origin matcher = bypass). *Consequence class: authorization bypass, not just a functional bug.* → `skills/04-review/cors/regression-guards/new-must-call-validate-and-init.md`.
- 🟡 **Every deserialization boundary crossed by lower-trust bytes needs a size limit before `from_slice`/`from_str`, and embedded regex compiles through `RegexBuilder::size_limit`/`dfa_size_limit`** (not bare `Regex::new`) — scale the bar to trust level. → `04-review/deserialization/`.
- 🟡 **CRD status conditions come from the fixed vocabulary** — `Accepted`, `ResolvedRefs`, `PartiallyInvalid` (routes, plus any resource with a partially-resolvable collection — e.g. the EdgionBackend credential pool), `Conflicted`, Gateway-only `Programmed`; there is deliberately no generic `Ready` (no per-resource data-plane ack channel). Don't invent a new condition type for a new resource. → `skills/01-architecture/01-controller/10-status-management.md`.
- 🟢 **Status shape follows attachment shape** — flat `status.conditions[]` for simple resources; per-`parentRef` for routes; GEP-2648 `status.ancestors[]` for multi-ancestor policies (`BackendTLSPolicy`, `EdgionBackendTrafficPolicy`). Pick by attachment cardinality, don't default a multi-attach resource to flat. → `10-status-management.md`.
- 🟢 **Application-layer field/key naming is camelCase** (spec fields, enum values, Secret data keys); the sole exception is K8s-native `kubernetes.io/tls` Secret keys (dotted). → `skills/02-features/03-resources/18-link-sys.md`.

## Z. DSL & plugin extension boundaries

> Detail: `skills/01-architecture/02-gateway/dsl-engine.md`, `skills/03-coding/dsl/`, `.../12-edgion-plugin-dev.md`.

- 🟡 **A new DSL capability follows orchestration-vs-computation** — orchestration (branch/compose/rewrite) may live in the language; real computation/protocol/integration becomes a Rust builtin (`BuiltinId`/`CallBuiltin` seam), never a new language feature (which re-opens every sandbox surface). → `dsl-engine.md` §0.
- 🔴 **A DSL builtin's error shape (silent-fallback / `TypeError` / `ApiError` / limit-error) is a frozen script-facing contract** chosen via the 4-question flow; changing it later is a breaking change needing a new lock-in test. Never call a fallible `Display`/formatter on user data unguarded (use `write!`, surface `Err` as `ApiError`). → `dsl/builtin-error-semantics.md`.
- 🔴 **`http.call()` re-checks every redirect hop (not just the initial address) when `httpBlockPrivateIps` is on, and disables proxy-env** (literal-IP only — DNS-rebinding still needs egress firewall); **`redis.*` re-resolves the LinkSys client on every call** (hot reload), and the runtime `ok=false` path stays implemented (the static alias check is best-effort). → `dsl/http-call.md`, `dsl/redis-builtin.md`.
- 🟡 **Adding a builtin/operation touches all layers in one PR** (`BuiltinId` + compiler resolution + async runtime arm + static validation + a lock-in test per layer). → `dsl/redis-builtin.md`.
- 🟡 **No Consumer model** — don't design plugin config around a Kong/APISIX `Consumer → Plugin` binding; credentials live in K8s Secrets referenced directly, identity travels via headers/ctx vars. → `12-edgion-plugin-dev.md`.

---

## Pre-commit self-check (fast pass)

Run the trigger-phrase checks in `AGENTS.md` §"Pre-commit checks". For design conformance,
scan your diff against the 🔴 rows above; the per-topic checklists live in
`skills/03-coding/SKILL.md`.
