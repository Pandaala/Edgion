---
name: webhook-request-templating
description: Webhook LinkSys provider after the target-unification + request-templating redesign — Service-or-URL target, allow-list request block, controller-side template compilation + secret resolution, conf_sync to gateway, consumer path_override API.
---

# Webhook provider — target unification + request templating

> The webhook LinkSys provider lets the `WebhookKeyGet` plugin (and other
> gateway callers) call an external HTTP service to resolve a value (a key, an
> identity, an enrichment field). This document describes the provider **after the
> target-unification + request-templating redesign**. For the field-by-field
> CRD schema see [02-features/03-resources/18-link-sys.md § `type: webhook`](../../02-features/03-resources/18-link-sys.md).
> Operator-facing consumption is the `WebhookKeyGet` plugin; downstream plugins
> read the written ctx var.

## What changed (redesign summary)

**Old model (removed):**
- `WebhookTarget` was an enum: `Uri { uri }` (raw URL) **or** `Service { backendRef, kind }`.
- The outbound request was constructed across **two layers**: the webhook resource defined a base `uri` + `requestMethod` + `defaultHeaders` + a tagged-enum `auth` block, and the *consuming* plugin supplied the rest (`pathSuffix` appended to the URI, `forwardHeaders` selecting which inbound headers to pass through). What actually went on the wire could not be audited from the webhook resource alone, and any consumer could forward any header by default.

**New model (current):**
- **`target` is a flat struct with two mutually-exclusive forms** (no tagged-enum wrapper): a Kubernetes Service reference — `{group, kind, name, namespace, port}` directly under `target`, no `backendRef:` nesting, the same fields HTTPRoute/TCPRoute use — **or** an origin-only external `url` (+ optional `blockPrivate`). `WebhookTarget::kind()` is the single source of truth for which form is set and enforces the exclusion (setting `url` together with any Service field is an admission error; setting neither is also an error). For the Service form, `port` is required; supported `kind`: `Service`, `ServiceClusterIp`, `ServiceEndpointSlice`, empty/unset (→ Service); `ExternalName`/`ServiceImport` are rejected at admission. The `url` form was reintroduced by the url-webhook-target feature: it must be origin-only (`scheme://host[:port]`, no path/query/fragment/credentials — the path comes from `request.path.template`), rejects `http://` combined with `tls.enabled: true`, and is SSRF-guarded (loopback/link-local/unspecified always blocked; `blockPrivate: true` also blocks RFC1918 + unique-local) with its own dedicated connection pool + DNS resolver.
- The webhook resource carries a required **`request` block** that is the *single source of truth* for the outbound request shape: `path`, `method`, **allow-listed** `args` / `headers` / `cookies`, and an optional `body`. Nothing is forwarded unless the resource names it.
- Each `args`/`headers`/`cookies` block splits into **`origin`** (copy the same-named value from the frozen incoming request, with a `presence` policy) and **`custom`** (operator-constructed: a literal `value`, or a `template`; secrets are expressed as `${secretRef:ns/name:key}` inside a template string, controller-resolved and carried inline).
- `request.body` (`BodySpec`, tagged on `type`) has three variants: `json` and `raw` carry a single `template` (`${type:name}` interpolation); `form` carries structured `fields` (same `CustomField` `value`/`template` source as headers/cookies), percent-encoded by the gateway. `json`/`form` auto-inject `Content-Type`; `raw` does not. See [02-features/03-resources/18-link-sys.md § `request.body`](../../02-features/03-resources/18-link-sys.md) for the full field-by-field contract, including the config-time JSON canary lint and the no-size-cap risk note.
- Templates reuse the existing **ctx-set template engine** (`${header:X}`, `${ctx:Y}`, ...), pre-compiled at resource ingest.

**Why:** unified target resolution (matches routes; ExternalName via a proxy Service), an allow-list so a misbehaving upstream plugin cannot leak headers, full auditability of the outbound request from the resource alone, and first-class `presence` semantics (required / warn / skip) for missing fields.

## End-to-end flow (controller → conf_sync → gateway)

```
  Controller (LinkSys manager)                       Gateway (WebhookRuntime)
┌──────────────────────────────────┐            ┌──────────────────────────────────┐
│ 1. Read WebhookServiceConfig      │            │ 5. ConfHandler<LinkSys> upserts   │
│    from K8s (CR ingest)           │            │    WebhookManager runtime         │
│ 2. validate_config():             │            │ 6. CompiledOutboundRequest::      │
│    - target (Service port/kind    │  conf_sync │    from_spec(request)             │
│      OR origin-only url)          │            │                                    │
│    - request template syntax,     │  (full     │    parses templates (already      │
│      origin/custom name conflicts │  struct,   │    parsed controller-side here too)│
│ 3. Scan ${secretRef:...} vars;    │  serde)    │ 7. webhook_call(session,          │
│    fetch; embed in resolved_      │───────────►│    path_override, resolved):      │
│    secrets; ship full struct      │            │    substitute secretRef vars from │
│ 4. (same struct carries resolved  │            │    resolved_secrets; evaluate      │
│    secret values inline)          │            │    templates vs SessionSnapshot   │
└──────────────────────────────────┘            │    templates vs SessionSnapshot   │
                                                 └──────────────────────────────────┘
```

### Controller side (LinkSys handler)

- Reads `WebhookServiceConfig` from the K8s CR. The LinkSys CRD is `x-kubernetes-preserve-unknown-fields: true`, so structural validation does **not** happen at the API server — the controller's `validate_config()` is the only admission gate, surfaced as the `Accepted` status condition.
- **Validation** (`edgion-resources/src/resources/link_sys/webhook.rs`): `backend_ref.port.is_some()`, `kind` in the supported set, and `request` field consistency (`validate_request_fields`, which delegates to the shared `OutboundRequestSpec::validation_error()` in `edgion-resources/src/resources/common/outbound_request.rs`): no template parse errors, no field name appearing in both `origin` and `custom`, no duplicate names within a block, no `headers.origin: [{name: Cookie}]` colliding with a non-empty `cookies` block, no explicit `Content-Type` header combined with `request.body` type `json`/`form`, and — for `request.body` type `json` — a config-time canary lint (every template variable substituted with a placeholder, then the result parsed as JSON) that rejects a template whose skeleton is not valid JSON, including an unquoted placeholder like `"count": ${ctx:n}`. Any violation → `Accepted=False` with a descriptive message; the resource is **not** loaded into the gateway runtime.
- **Template/secret errors propagate**: a resource that fails compilation is not registered; a consumer calling it hits the "webhook not found" path (same as `statusOnError`).

> **Secret resolution in custom templates.** Secret values in `request.*.custom` fields
> (including `request.body` — `json`/`raw` templates and `form` field templates) are
> expressed as `${secretRef:ns/name:key}` inside a `template` string (e.g.,
> `template: "Bearer ${secretRef:prod/webhook-token:token}"`). The controller
> (`SystemConfig::Webhook` arm) scans `collect_secret_ref_names`, registers each ref in
> `SecretRefManager`, fetches the Secret via `fetch_secret` (ReferenceGrant enforced for
> cross-namespace), and embeds the resolved values in `spec.resolved_secrets` (a
> `ResolvedSecrets` newtype; admin `/configclient` renders entries as `"[redacted]"`). The
> full struct is shipped over conf_sync; the gateway substitutes `${secretRef:...}` vars from
> `resolved_secrets` at call time. A Secret change triggers `trigger_cascading_requeue` → full
> re-resolve → re-emit (no operator action required on rotation).

### conf_sync

- The gateway receives the **full** `WebhookServiceConfig` struct (serde) — the standard LinkSys conf_sync path. The compiled request templates are re-derived gateway-side from the synced `request` block (no separate "compiled" wire format). Resolved secret values ride along in `spec.resolved_secrets` (a `ResolvedSecrets` newtype); admin `/configclient` output renders each entry as `"[redacted]"`.
- The gateway compiles `CompiledOutboundRequest` once at upsert time, **not** per call — keeping the request-time (RT) loop free of template parsing. `${secretRef:...}` substitution happens at call time from the `resolved_secrets` passed through `webhook_call`.

### Gateway side (WebhookRuntime)

- `WebhookManager.upsert()` (`providers/webhook/manager.rs`) builds a `WebhookRuntime` carrying a `compiled_request: CompiledOutboundRequest` field.
- `CompiledRequest::from_spec(spec)` (`edgion-gateway/src/plugins/http/common/outbound/request.rs`) compiles every `path`/`method`/`args`/`headers`/`cookies` `custom.template`, and the optional `request.body` (`json`/`raw` template, or `form` field templates), into a `CompiledTemplate`. `${secretRef:ns/name:key}` vars in templates are NOT resolved here; they are stored as-is and substituted at call time from the `resolved_secrets` map carried in the spec.
- At call time, `webhook_call(runtime, session, path_override, resolved, global_max_bytes)` (`providers/webhook/call.rs`) calls `compiled_request.build_request(session, path_override, base_url, resolved)`, substituting `${secretRef:...}` vars from the passed `resolved` map, then handles retry / health / rate-limit / response-size cap. The outbound `reqwest::Request` is built once and cloned per retry.

## Consumer API change

The consumer-facing entry is `resolve_webhook_key()` (`providers/webhook/resolver.rs`), called by the `WebhookKeyGet` plugin.

**Old:** consumer passed `forward_headers` + `path_suffix` (appended to the webhook's base URI), plus method/headers/body.

**New:** consumer passes only `session` (a `SessionSnapshot` for origin lookups and `${...}` template evaluation) and an optional `path_override`.

```rust
// edgion-resources/.../plugin_configs/webhook_key_get.rs — WebhookVar
WebhookVar {
    name: String,
    webhook_ref: String,
    path_override: Option<String>,   // replaces forward_headers + path_suffix
    extract: WebhookExtract,         // response-side extractor — unchanged
    default: Option<String>,
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
- Consumers that need dynamic per-call values set them into the gateway context first (via a preceding CtxSet entry), then WebhookKeyGet runs and the webhook templates read `${ctx:NAME}`. CtxSet local vars and WebhookKeyGet vars must not share one plugin entry. No new "invoker arg" concept was added.

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
| `${remoteIp}` | resolved client IP |
| `${secretRef:ns/name:key}` | Secret value resolved by the controller at config-apply time; shipped inline in `resolved_secrets`; substituted at call time. Admin `/configclient` renders as `"[redacted]"`. Both `ns` and `key` are required. |

**`origin` vs `template` for the same field:** `origin: [{name: X}]` copies `X` from the **frozen entry-time snapshot** (immune to earlier-plugin rewrites). `custom: [{name: X, template: "${header:X}"}]` reads the **current view**. Use `origin` for the unmodified client value; use a `${header:X}` template for the post-transformation value.

## Presence policy semantics

`presence` lives only on `origin` fields (`custom` fields always carry a value). Enum `PresencePolicy`:

| Value | Behavior when the origin field is absent from the incoming request |
|-------|--------------------------------------------------------------------|
| `skipIfMissing` (**default**) | field omitted from the outbound request, no log |
| `warnIfMissing` | field omitted, a PluginLog alert is written, request continues |
| `required` | the webhook call is treated as **failed**, and WebhookKeyGet writes `default` or leaves the target variable unset |

The shared builder records missing warning fields in the caller-owned, stack-only
`OutboundRequestDiagnostics`. ForwardAuth and WebhookKeyGet consume it into their
PluginLog after the call, before classifying the outcome. Each logical call emits at
most one `warn outbound field missing` entry, without names or values, subject to
the existing PluginLog capacity limit. Retries do not repeat it. Evidence observed
before a later failure is retained; gates before field evaluation produce none.
See the [diagnostic regression guard](../../04-review/observability/regression-guards/outbound-presence-diagnostics.md)
for the consumer inventory and verification boundaries.

Note: webhook connector has no fail-open mode. When a webhook call fails for any reason, `resolve_webhook_key` returns `None`, and WebhookKeyGet writes `default` or leaves the var unset — there is no `statusOnError` or automatic bypass.

## Health state semantics

The single invariant carried over from the prior endpoint-decouple work, preserved here:

- **`request.*` config change → `upsert()` → health reset.** Editing `request.headers` (or any other `request.*` / target field) rebuilds the `WebhookRuntime` with a fresh `compiled_request` and **fresh** health atomics (`healthy`, passive counters). This is intentional: an operator-initiated config change is not pod churn.
- **EndpointSlice churn → health preserved.** EndpointSlice changes do **not** trigger LinkSys re-emit, do **not** call `upsert()`, and do **not** reset health. Endpoint resolution happens at call time from `EpSliceStore` (`runtime.resolved_target.pick_url()`), so the readiness set can change without disturbing aggregate webhook health.

Tests: `manager.rs::upsert_resets_health_on_request_change` (reset path) and the prior decouple regression test (preserve-across-churn path).

## Migration (operator-facing)

The current contract uses only the replacement fields below. The code still
contains ignored `auth` / `defaultHeaders` / `requestMethod` placeholders from
an earlier migration; those placeholders violate the project's pre-release
no-compatibility policy. Treat them as removal debt, not supported input: do not
document, extend, or copy the pattern into new schemas. A future code change
should remove the fields and their warning/tests atomically.

Migration map:

| Old | New |
|-----|-----|
| `target: { type: uri, uri: "https://ext..." }` | `target: { url: "https://ext...", blockPrivate: true }` — the URL form was reintroduced (origin-only, SSRF-guarded); no proxy `Service` needed |
| `target: { type: service, backendRef, kind }` | `target: { kind, name, port, ... }` (flat — no `backendRef:` nesting) |
| `requestMethod: POST` | `request.method.template: "POST"` |
| `defaultHeaders: { X-Source: edgion }` | `request.headers.custom: [{ name: X-Source, value: edgion }]` |
| `auth.bearer: { secretRef: { name: N, namespace: NS }, key: token }` | `request.headers.custom: [{ name: Authorization, template: "Bearer ${secretRef:NS/N:token}" }]` |
| `auth.apiKey: { secretRef: { name: N, namespace: NS }, ..., headerName: X-API-Key }` | `request.headers.custom: [{ name: X-API-Key, template: "${secretRef:NS/N:apiKey}" }]` |
| consumer `pathSuffix` / `forwardHeaders` | declare on the webhook resource (`request.path`, `request.headers.origin`); use `pathOverride` only for genuinely per-call paths |

## Module map

```
edgion-resources/src/resources/link_sys/webhook.rs   # WebhookServiceConfig, WebhookTarget,
                                                      #   WebhookOutboundRequest, PathSpec, MethodSpec,
                                                      #   ListFields, OriginField, PresencePolicy,
                                                      #   CustomField, CustomSource, ResolvedSecrets,
                                                      #   validate_config / validate_request_fields
edgion-resources/src/resources/edgion_plugins/
    plugin_configs/webhook_key_get.rs                # WebhookKeyGetConfig / WebhookVar / WebhookExtract
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
- **Presence policies** — `skipIfMissing` (silent), `warnIfMissing` (alert + continue), `required` (webhook failure; consuming plugin handles fallback).
- **Path override** — honored only with `path.allowOverride: true`; ignored otherwise.
- **ctx-set integration** — CtxSet writes local vars first; WebhookKeyGet runs after; webhook templates read `${ctx:...}`.
- **WebhookKeyGet then KeyAuth** — WebhookKeyGet writes a ctx key; KeyAuth reads `{ type: ctx, name: … }`. Reverse order is a silent auth bypass.
- **Template substitution** — literal, single-var, multi-var, frozen-path/method reuse when `path`/`method` omitted.
- **Health semantics** — `request.*` edit resets health; EndpointSlice churn preserves it.

Unit tests: `compiled_request.rs` (compilation + `build_request`) and `manager.rs` (upsert, invalid-template rejection, health reset). See [05-testing/01-integration-testing.md](../../05-testing/01-integration-testing.md) for running the suite.
