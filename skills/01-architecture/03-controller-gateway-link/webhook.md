---
name: webhook-request-templating
description: Webhook LinkSys provider after the target-unification + request-templating redesign — backendRef-only target, allow-list request block, controller-side template compilation + secret resolution, conf_sync to gateway, consumer path_override API.
---

# Webhook provider — target unification + request templating

> The webhook LinkSys provider lets gateway plugins (ctx-set, key-auth, DSL, ...)
> call an external HTTP service to resolve a value (a key, an identity, an
> enrichment field). This document describes the provider **after the
> target-unification + request-templating redesign**. For the field-by-field
> CRD schema see [02-features/03-resources/18-link-sys.md § `type: webhook`](../../02-features/03-resources/18-link-sys.md).

## What changed (redesign summary)

**Old model (removed):**
- `WebhookTarget` was an enum: `Uri { uri }` (raw URL) **or** `Service { backendRef, kind }`.
- The outbound request was constructed across **two layers**: the webhook resource defined a base `uri` + `requestMethod` + `defaultHeaders` + a tagged-enum `auth` block, and the *consuming* plugin supplied the rest (`pathSuffix` appended to the URI, `forwardHeaders` selecting which inbound headers to pass through). What actually went on the wire could not be audited from the webhook resource alone, and any consumer could forward any header by default.

**New model (current):**
- **`target` is `backendRef`-only** — a single `BackendObjectReference` (`{group, kind, name, namespace, port}`), the same shape HTTPRoute/TCPRoute use. No `Uri` variant, no `type: service` wrapper tag. `port` is required. Supported `kind`: `Service`, `ServiceClusterIp`, `ServiceEndpointSlice`, empty/unset (→ Service). `ExternalName`/`ServiceImport` are rejected at admission.
- The webhook resource carries a required **`request` block** that is the *single source of truth* for the outbound request shape: `path`, `method`, and **allow-listed** `args` / `headers` / `cookies`. Nothing is forwarded unless the resource names it.
- Each `args`/`headers`/`cookies` block splits into **`origin`** (copy the same-named value from the frozen incoming request, with a `presence` policy) and **`custom`** (operator-constructed: a literal `value`, a `template`, or a K8s `secret`).
- Templates reuse the existing **ctx-set template engine** (`${header:X}`, `${ctx:Y}`, ...), pre-compiled at resource ingest.

**Why:** unified target resolution (matches routes; ExternalName via a proxy Service), an allow-list so a misbehaving upstream plugin cannot leak headers, full auditability of the outbound request from the resource alone, and first-class `presence` semantics (required / warn / skip) for missing fields.

## End-to-end flow (controller → conf_sync → gateway)

```
  Controller (LinkSys manager)                       Gateway (WebhookRuntime)
┌──────────────────────────────────┐            ┌──────────────────────────────────┐
│ 1. Read WebhookServiceConfig      │            │ 5. ConfHandler<LinkSys> upserts   │
│    from K8s (CR ingest)           │            │    WebhookManager runtime         │
│ 2. validate_config():             │            │ 6. CompiledOutboundRequest::      │
│    - target.backendRef (port/kind)│  conf_sync │    from_spec(request, resolve)    │
│    - request template syntax,     │  (full     │    parses templates (already      │
│      origin/custom name conflicts │  struct,   │    parsed controller-side here too)│
│ 3. Register secretRef in          │  serde)    │    + reads resolved secret values │
│    SecretRefManager               │───────────►│    from the global SecretStore    │
│ 4. Embed resolved Secret data     │            │ 7. webhook_call(session,          │
│    into spec; ship full struct    │            │    path_override): evaluate        │
└──────────────────────────────────┘            │    templates vs SessionSnapshot   │
                                                 └──────────────────────────────────┘
```

### Controller side (LinkSys handler)

- Reads `WebhookServiceConfig` from the K8s CR. The LinkSys CRD is `x-kubernetes-preserve-unknown-fields: true`, so structural validation does **not** happen at the API server — the controller's `validate_config()` is the only admission gate, surfaced as the `Accepted` status condition.
- **Validation** (`edgion-resources/src/resources/link_sys/webhook.rs`): `backend_ref.port.is_some()`, `kind` in the supported set, and `request` field consistency (`validate_request_fields`): no template parse errors, no field name appearing in both `origin` and `custom`, no duplicate names within a block, no `headers.origin: [{name: Cookie}]` colliding with a non-empty `cookies` block. Any violation → `Accepted=False` with a descriptive message; the resource is **not** loaded into the gateway runtime.
- **Template/secret errors propagate**: a resource that fails compilation is not registered; a consumer calling it hits the "webhook not found" path (same as `statusOnError`).

> **Implementation status — secret resolution (`CustomSource::Secret`).** The other LinkSys
> credentials (Redis/Etcd/ES auth, TLS) are resolved by the controller's credential pipeline
> (`AuthSpec`/`TlsSpec` traits) which registers each `secretRef` in `SecretRefManager` and embeds
> the resolved `Secret` into the spec for conf_sync. The new webhook `request.*.custom.secret`
> refs are **not yet on that path**: the LinkSys handler currently *skips* webhook auth
> processing and only emits a WARN for the deprecated `auth`/`defaultHeaders`/`requestMethod`
> shim fields (see `edgion-controller/.../handlers/link_sys.rs`, `SystemConfig::Webhook` arm —
> "Auth processing will be re-implemented … with the new request field"). On the gateway,
> `CompiledOutboundRequest::from_spec` resolves each `CustomSource::Secret` through the
> `resolve_secret(ns, name, key)` callback, which in production reads the global `SecretStore`
> via `edgion_common::secret::get_secret`. Because `Secret` is a `no_sync_kind`
> (`ResourceKind::Secret` has no gateway ClientCache), that store is **not** populated over
> conf_sync today, so the request-secret flow is not yet end-to-end — the
> `11-custom-secret.yaml` integration fixture exercises the *intended* behavior. The design
> target is: controller registers the ref in `SecretRefManager`, resolves it, ships the value
> to the gateway, and a referenced-Secret change triggers `trigger_cascading_requeue` → full
> re-resolve → re-emit (no operator action on rotation, same model as the old
> `WebhookAuth::Bearer`). Treat secret resolution as the remaining wiring, not a finished path.

### conf_sync

- The gateway receives the **full** `WebhookServiceConfig` struct (serde) — the standard LinkSys conf_sync path. The compiled request templates are re-derived gateway-side from the synced `request` block (no separate "compiled" wire format). Secret *values* are intended to ride along via the controller-embed mechanism (see the implementation-status note above); resolved secret material in admin `/configclient` output renders as `"[redacted]"`.
- The gateway compiles `CompiledOutboundRequest` once at upsert time, **not** per call — keeping the request-time (RT) loop free of template parsing and secret lookups.

### Gateway side (WebhookRuntime)

- `WebhookManager.upsert()` (`providers/webhook/manager.rs`) builds a `WebhookRuntime` carrying a `compiled_request: CompiledOutboundRequest` field.
- `CompiledOutboundRequest::from_spec(spec, resolve_secret)` (`providers/webhook/compiled_request.rs`) compiles every `path`/`method`/`custom.template` into a `CompiledTemplate` and resolves every `CustomSource::Secret` to a plain string via the `resolve_secret(ns, name, key)` callback (production: `edgion_common::secret::get_secret`). The optional `prefix` (e.g. `"Bearer "`) is applied here at load time.
- At call time, `webhook_call(runtime, session, path_override, global_max_bytes)` (`providers/webhook/call.rs`) calls `compiled_request.build_request(session, path_override, base_url)`, then handles retry / health / rate-limit / response-size cap. The outbound `reqwest::Request` is built once and cloned per retry.

## Consumer API change

The consumer-facing entry is `resolve_webhook_key()` (`providers/webhook/resolver.rs`), dispatched from `session_adapter` on a `KeyGet::Webhook` key.

**Old:** consumer passed `forward_headers` + `path_suffix` (appended to the webhook's base URI), plus method/headers/body.

**New:** consumer passes only `session` (a `SessionSnapshot` for origin lookups and `${...}` template evaluation) and an optional `path_override`.

```rust
// edgion-resources/src/common/key_accessor.rs — KeyGet::Webhook variant
KeyGet::Webhook {
    webhook_ref: String,
    path_override: Option<String>,   // replaces forward_headers + path_suffix
    extract: WebhookExtract,         // response-side extractor — unchanged
}

// edgion-gateway/.../webhook/resolver.rs — consumer entry
pub async fn resolve_webhook_key(
    session: &dyn PluginSession,
    webhook_ref: &str,
    _forward_headers: Option<&[String]>,   // retained in signature, no longer consumed
    path_override: Option<&str>,
    extract: &WebhookExtract,
) -> Option<String>
```

- `forward_headers` / `path_suffix` are **removed** from consumer config. Headers to forward must be declared on the webhook resource's `request.headers.origin` allow-list; per-call paths use `path_override`.
- `path_override` is **only honored** when the webhook resource declares `request.path.allowOverride: true`. Otherwise it is silently ignored (not an error).
- `extract` (response-side header/cookie/body extractor) is **kept** — it parses the response, unrelated to outbound construction.
- Consumers that need dynamic per-call values set them into the gateway context first (via ctx-set `vars`), then reference them from the webhook templates with `${ctx:NAME}`. No new "invoker arg" concept was added.

## Template engine reference

Webhook templates reuse the ctx-set engine (`CompiledTemplate` in `edgion-resources/src/resources/edgion_plugins/plugin_configs/ctx_set.rs`), compiled at ingest, evaluated against the `PluginSession`/`SessionSnapshot` at call time.

| Token | Meaning |
|-------|---------|
| `${header:Name}` | incoming request header — **current** (possibly rewritten) view |
| `${query:name}` | incoming request query param |
| `${cookie:name}` | incoming request cookie |
| `${ctx:name}` | gateway context variable (set by ctx-set or other plugins); undefined → empty string |
| `${path}` / `${uri}` | current (possibly rewritten) path / URI |
| `${requestPath}` / `${requestUri}` | frozen entry-time path / URI |
| `${method}` | current HTTP method |
| `${clientIp}` | real client IP |

**`origin` vs `template` for the same field:** `origin: [{name: X}]` copies `X` from the **frozen entry-time snapshot** (immune to earlier-plugin rewrites). `custom: [{name: X, template: "${header:X}"}]` reads the **current view**. Use `origin` for the unmodified client value; use a `${header:X}` template for the post-transformation value.

## Presence policy semantics

`presence` lives only on `origin` fields (`custom` fields always carry a value). Enum `PresencePolicy`:

| Value | Behavior when the origin field is absent from the incoming request |
|-------|--------------------------------------------------------------------|
| `skipIfMissing` (**default**) | field omitted from the outbound request, no log |
| `warnIfMissing` | field omitted, a PluginLog alert is written, request continues |
| `required` | the webhook call is treated as **failed** (`statusOnError` / `allowDegradation` apply exactly as for any other webhook error) |

`presence` and `allowDegradation` are **orthogonal**: `presence: required` decides *whether a missing field counts as a failure*; `allowDegradation` then decides whether that failure aborts the plugin chain (`false`, default → `statusOnError` to client) or silently skips the webhook and continues (`true` → degraded mode, not 5xx).

## Health state semantics

The single invariant carried over from the prior endpoint-decouple work, preserved here:

- **`request.*` config change → `upsert()` → health reset.** Editing `request.headers` (or any other `request.*` / target field) rebuilds the `WebhookRuntime` with a fresh `compiled_request` and **fresh** health atomics (`healthy`, passive counters). This is intentional: an operator-initiated config change is not pod churn.
- **EndpointSlice churn → health preserved.** EndpointSlice changes do **not** trigger LinkSys re-emit, do **not** call `upsert()`, and do **not** reset health. Endpoint resolution happens at call time from `EpSliceStore` (`runtime.resolved_target.pick_url()`), so the readiness set can change without disturbing aggregate webhook health.

Tests: `manager.rs::upsert_resets_health_on_request_change` (reset path) and the prior decouple regression test (preserve-across-churn path).

## Migration (operator-facing)

The redesign carries a one-release **shim**: removed fields `auth` / `defaultHeaders` / `requestMethod` are accepted as ignored `Option<serde_json::Value>` no-ops so an old-controller → new-gateway rolling upgrade deserializes cleanly; the controller logs a WARN naming them. They are removed next release (controller-first ordering required from then on). Migration map:

| Old | New |
|-----|-----|
| `target: { type: uri, uri: "https://ext..." }` | create a proxy `Service` for the external endpoint, then `target: { backendRef: { kind: Service, name: ..., port: ... } }` |
| `target: { type: service, backendRef, kind }` | `target: { backendRef: { kind, name, port, ... } }` |
| `requestMethod: POST` | `request.method.template: "POST"` |
| `defaultHeaders: { X-Source: edgion }` | `request.headers.custom: [{ name: X-Source, value: edgion }]` |
| `auth.bearer: { secretRef, key: token }` | `request.headers.custom: [{ name: Authorization, secret: { secretRef, key: token, prefix: "Bearer " } }]` |
| `auth.apiKey: { ..., headerName: X-API-Key }` | `request.headers.custom: [{ name: X-API-Key, secret: { secretRef, key: apiKey } }]` |
| consumer `pathSuffix` / `forwardHeaders` | declare on the webhook resource (`request.path`, `request.headers.origin`); use `pathOverride` only for genuinely per-call paths |

## Module map

```
edgion-resources/src/resources/link_sys/webhook.rs   # WebhookServiceConfig, WebhookTarget,
                                                      #   WebhookOutboundRequest, PathSpec, MethodSpec,
                                                      #   ListFields, OriginField, PresencePolicy,
                                                      #   CustomField, CustomSource, SecretSpec,
                                                      #   validate_config / validate_request_fields
edgion-resources/src/common/key_accessor.rs          # KeyGet::Webhook { webhook_ref, path_override, extract }
edgion-gateway/src/link_sys/providers/webhook/
├── manager.rs          # WebhookManager.upsert() — builds runtime, resolves secrets, resets health
├── compiled_request.rs # CompiledOutboundRequest::{from_spec, build_request}
├── call.rs             # webhook_call(runtime, session, path_override, max_bytes)
├── resolver.rs         # resolve_webhook_key() — consumer entry, response extract
├── runtime.rs          # WebhookRuntime (compiled_request + health atomics + resolved_target)
├── health.rs           # active/passive health
└── client.rs           # reqwest client construction (TLS material)
```

## Testing

Integration suite: `edgion-tests/src/client/suites/edgion_plugins/webhook_key_get/`
(fixtures under `edgion-tests/integration/conf/EdgionPlugins/WebhookKeyGet/`). Key scenarios:

- **Target resolution** — `kind: Service`/`ServiceEndpointSlice` (call-time EndpointSlice path) and `kind: ServiceClusterIp` (ClusterIP DNS path).
- **Origin allow-list** — only named `origin` fields forwarded; unnamed inbound headers/args/cookies are dropped.
- **Custom fields** — `value` (literal), `template` (`${ctx:...}` / `${header:...}`), `secret` (resolved + `prefix`).
- **Presence policies** — `skipIfMissing` (silent), `warnIfMissing` (alert + continue), `required` (5xx on missing), plus `required` + `allowDegradation: true` (degrade, not 5xx; PluginLog records cause).
- **Path override** — honored only with `path.allowOverride: true`; ignored otherwise.
- **ctx-set integration** — consumer sets `ctx` vars, webhook template reads `${ctx:...}`.
- **Template substitution** — literal, single-var, multi-var, frozen-path/method reuse when `path`/`method` omitted.
- **Health semantics** — `request.*` edit resets health; EndpointSlice churn preserves it.

Unit tests: `compiled_request.rs` (compilation + `build_request`) and `manager.rs` (upsert, invalid-template rejection, health reset). See [05-testing/01-integration-testing.md](../../05-testing/01-integration-testing.md) for running the suite.
