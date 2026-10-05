# Access Log

> **🔌 Edgion extension**
> 
> The access log format and plugin log structure are Edgion Gateway features; they are not part of standard Gateway API.

Edgion Gateway emits one structured JSON access log entry per request, recording the full request lifecycle: client information, route matching, backend responses, per-plugin execution logs, and errors. It is the gateway's primary mechanism for diagnosing individual requests.

## Overview

The access log is the right tool when you need to answer a question about a *specific* request — "what happened to the request with `x-trace-id=abc`?". Aggregate metrics (see [Metrics](./metrics.md)) answer fleet-wide questions; the access log answers per-request ones.

Each entry is one JSON object on a single line, so the file is directly consumable by `jq`, Loki, Elasticsearch, or any line-oriented log pipeline.

```
client request ──► Gateway ──► backend
                     │
                     └──► one JSON line ──► logs/edgion_access.log
```

## Configuration

The access log is configured under the `access_log` section of the gateway configuration.

### Minimal configuration

```yaml
access_log:
  enabled: true
  output:
    localFile:
      path: logs/edgion_access.log
```

This enables the log with default queue and rotation behavior. `path` is resolved relative to `work_dir` unless absolute.

### Production configuration

```yaml
access_log:
  enabled: true
  output:
    localFile:
      path: logs/edgion_access.log
      # In-memory queue before backpressure applies to the request path
      queue_size: 80000
      rotation:
        strategy: daily
        max_files: 7
```

### Configuration parameters

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `enabled` | Boolean | `false` | Enable the access log channel |
| `output.localFile.path` | String | none | Log file path (relative to `work_dir` unless absolute). **Required** when `localFile` is used |
| `output.localFile.queue_size` | Integer | none | In-memory queue size before backpressure applies |
| `output.localFile.rotation.strategy` | Enum | none | Rotation strategy. Example value: `daily` |
| `output.localFile.rotation.max_files` | Integer | none | Maximum rotated files to keep |
| `output.es` | Object | none | Ship the access log to Elasticsearch (via `link_sys`). Set `linkSysRef.namespace` and `linkSysRef.name` to reference the target LinkSys resource |

The raw client-entry URI (`request_uri`) is controlled by a separate top-level `access_log_options` section, not by the `access_log` block — see [Special fields](#special-fields).

> The `output` block also accepts an `es` sink. When its Elasticsearch producer→bulk-loop channel is full, the non-blocking access-log submission is dropped and `edgion_access_log_dropped_total` increments. See [Metrics](./metrics.md).

## Degradation: fallback sink and backfill

### Fallback sink

The `access_log` block accepts an optional `fallback` field alongside `output`. When the primary sink's `healthy()` check returns false (for example during an Elasticsearch or Kafka outage), new entries are routed to the fallback instead. One log entry lands in **at most one sink** — there is no fan-out.

The typical pattern is an Elasticsearch primary with a bounded LocalFile fallback so access logs survive an outage without extra infrastructure:

```yaml
access_log:
  enabled: true
  output:
    es:
      linkSysRef:
        namespace: default
        name: es-main
  fallback:
    localFile:
      path: logs/es_access_fallback.log
      rotation:
        strategy: { size: 104857600 }   # 100 MB per file
        maxFiles: 5                       # hard cap ~500 MB
      diskUsageThresholdPercent: 90
```

**Bounding the fallback disk usage (recommended).** An unrotated fallback file can fill the disk during a long outage. Limit growth with `rotation.strategy: { size: <bytes> }` combined with `rotation.maxFiles` (total cap ≈ size × maxFiles). `diskUsageThresholdPercent` (default 90) causes the sink to mark itself unhealthy — and shed entries — when the host volume crosses that percentage. This acts as a last-resort disk guard when the fallback is the final sink in the chain. To disable the disk check entirely, set `diskUsageThresholdPercent: 0`.

### Fallback does not auto-backfill

Entries that land in the fallback LocalFile during an outage stay there. When the primary recovers, the gateway resumes writing new entries to it automatically, but the fallback file is **not** replayed. To recover the missed records after an outage, trigger a manual backfill.

### Backfill endpoint

```
POST /api/v1/access-log/backfill
Authorization: Bearer <admin-token>
```

This endpoint requires the same admin bearer token as all other `/api/v1/` admin routes.

**Preconditions** — the endpoint returns an error immediately if any condition is not met:

| HTTP status | Condition |
|-------------|-----------|
| `400` | The primary sink is not Elasticsearch |
| `400` | No LocalFile fallback is configured |
| `409` | Elasticsearch is not currently healthy — retry after recovery |
| `503` | The access logger is not initialized |

**Success response (`200`)**

```json
{
  "success": true,
  "data": {
    "summary": {
      "files_processed": 2,
      "lines_backfilled": 41,
      "lines_failed": 0,
      "files_remaining": 0,
      "stopped_reason": null
    }
  }
}
```

**Behavior details**

- Reads each fallback file line by line (one ES document per line) and re-ingests into Elasticsearch via the normal bulk channel.
- After processing a file, renames it to `<name>.backfilled-<epoch_secs>`. Subsequent triggers skip already-renamed files, making the operation **idempotent at file granularity**.
- **At-least-once delivery warning**: the batch that was in progress when the outage started may appear duplicated in Elasticsearch — no stable document ID is used and no deduplication is performed. Access logs tolerate this level of duplication, but be aware when querying aggregate counts.
- **Index placement**: backfilled documents are indexed under the **current date's** index (`<prefix>-<today>`), not the date the request occurred — re-ingest uses the same bulk path as live traffic, which derives the index from the current time. Each document still carries its own `@timestamp`, so a time-range query over the `<prefix>-*` index pattern finds them; only date-specific index queries or per-daily-index retention/ILM are affected.
- The endpoint feeds the existing ES bulk channel and applies natural back-pressure. Trigger it **off-peak** to avoid adding bulk-ingest load on top of live traffic.

### Backfill metrics

Two counters track backfill progress alongside the existing `edgion_link_sys_es_bulk_channel_full_total` back-pressure signal:

| Metric | Description |
|--------|-------------|
| `edgion_access_log_backfill_ingested_total` | Lines re-queued into the Elasticsearch bulk channel during a backfill (final ES indexing happens asynchronously in the bulk loop) |
| `edgion_access_log_backfill_failed_total` | Lines that could not be enqueued to the bulk channel (channel closed); the fallback file is kept for a later retry |

## Log format

The log is always JSON, one entry per line. Optional fields are omitted entirely when empty (rather than emitted as `null`), which keeps each line compact. The top-level shape is:

```json
{
  "ts": 1767089146376,
  "request_info": {
    "trace_id": "4bf92f3577b34da6a3ce929d0e0e4736",
    "direct_peer_ip": "192.168.1.20",
    "host": "api.example.com",
    "path": "/health",
    "status": 200
  },
  "match_info": { },
  "backend_context": { },
  "genai": { },
  "stage_logs": [ ],
  "diagnostics": [
    { "kind": "info", "code": "AcmeChallengeServed" },
    { "kind": "error", "code": "BackendRequestTimeoutRetry" }
  ]
}
```

Request-scoped fields (`direct_peer_ip`, `host`, `path`, `status`, etc.) nest one level deep under `request_info`; routing, backend, and plugin information sit alongside it at the top level.

## Field reference

### Top-level fields

| Field | Type | Description |
|-------|------|-------------|
| `ts` | Integer | Entry timestamp, epoch milliseconds |
| `request_info` | Object | Request-scoped fields — see [Request fields](#request-fields) below |
| `match_info` | Object | Matched route identity (omitted when no route matched) |
| `backend_context` | Object | Backend identity and upstream timing (omitted/`null` when no backend was used) |
| `genai` | Object | Final GenAI usage and bounded per-attempt accounting (omitted for non-AI requests) |
| `stage_logs` | Array\<Object\> | Per-plugin execution logs grouped by stage (omitted when empty) |
| `diagnostics` | Array\<Object\> | Ordered request diagnostics as `{kind, code}` entries; `kind` is `info` or `error` (omitted when empty) |
| `diagnostics_full` | Boolean | `true` when the bounded diagnostics buffer dropped one or more tail entries (omitted otherwise) |
| `ext` | Object | Operator-declared fields from [`AccessLogExtern`](#extending-the-log-accesslogextern) (omitted when nothing was collected) |

### Request fields

Nested one level deep under the top-level `request_info` object (see [Top-level fields](#top-level-fields) above) — not flattened into the entry's top level. Optional fields are omitted when absent; `status` is always present and uses `0` for an unknown final outcome.

| Field | Type | Description |
|-------|------|-------------|
| `direct_peer_ip` | String | Physical downstream transport peer (`DirectPeerIp`) |
| `client-port` | Integer | Client TCP port |
| `remote_ip` | String | Resolved client IP (`RemoteIp`) after the applicable proxy/RealIp resolution |
| `trace_id` | String | Canonical W3C trace ID, 32 lowercase hexadecimal characters |
| `request_ids["<custom-header>"]` | String | Active custom ID in the request map; copied external values follow the log masking policy |
| `host` | String | Request `Host` header |
| `path` | String | Normalized request path |
| `status` | Integer | Final downstream HTTP status, equivalent to Nginx/OpenResty `$status`; `0` means no final status was observable |
| `x-forwarded-for` | String | `X-Forwarded-For` header value (when present) |
| `discover_protocol` | String | Auto-detected protocol. Example values: `grpc`, `websocket` |
| `grpc_service` | String | gRPC service name (gRPC requests only) |
| `grpc_method` | String | gRPC method name (gRPC requests only) |
| `sni` | String | TLS Server Name Indication (TLS connections only) |
| `tls_id` | String | TLS connection/session identifier (TLS connections only) |
| `request_uri` | String | Client-entry path and query before normalization or rewrites; emitted **only** when `access_log_options.includeRawUri: true` |

### `match_info` fields

| Field | Type | Description |
|-------|------|-------------|
| `rns` | String | Route namespace |
| `rn` | String | Route name |
| `rule_id` | Integer | Matched rule index (starts at 0) |
| `match_id` | Integer | Matched match-item index (starts at 0) |
| `sv` | Integer | Controller→Gateway sync version; omitted when 0 |

When no route matches, the entire `match_info` object is omitted.

### `backend_context` fields

| Field | Type | Description |
|-------|------|-------------|
| `name` | String | Backend service name |
| `namespace` | String | Backend service namespace |
| `upstreams` | Array\<Object\> | One entry per upstream attempt (retries produce multiple entries) |

#### `upstreams[]` sub-fields

| Field | Type | Unit | Description |
|-------|------|------|-------------|
| `ip` | String | — | Upstream IP address |
| `port` | Integer | — | Upstream port |
| `status` | Integer | — | Upstream response status code |
| `ct` | Integer | ms | Connect time |
| `ht` | Integer | ms | Header time (time to first response byte) |
| `bt` | Integer | ms | Body first-byte time |
| `et` | Integer | ms | Total elapsed time for this attempt |
| `err` | Array\<String\> | — | Error/reason labels for this attempt (omitted when empty). Retry decisions are labeled explicitly: connect failures carry the error type (`ConnectTimedout`, `TLSHandshakeTimedout`, `ConnectRefused`, ...), per-try timeouts carry `ReadTimedout`/`WriteTimedout`, status-code retries carry `RetryOnStatus`, stale-keepalive retries carry `ConnectionReuseRetry` |

When no backend was used (route not matched, or the request was short-circuited by a plugin), `backend_context` is `null` or omitted.

### `genai` fields

The final request summary contains `provider`, `model`, `input_tokens`, `output_tokens`, optional
cache-token counts and `estimated_cost_usd`, `streaming`, `usage_estimated`, optional `quota_status`
and `ttft_ms`, plus the per-attempt fields below. `attempts` contains at most eight dispatched
attempts and its serialized section is capped at 8 KiB. `attempts_truncated` is present and `true`
when later summaries were omitted to preserve either bound.

| `attempts[]` field | Type | Description |
|--------------------|------|-------------|
| `number` | Integer | One-based dispatched-attempt number |
| `service` | String | Bounded namespaced EdgionBackend key (`namespace/name`) |
| `provider` | String | Fixed provider enum |
| `credential` | String | Non-secret configured credential alias; never provider credential or Secret data |
| `status` | Integer | Provider HTTP status, when received |
| `failure` | String | Fixed class: `retryable_status`, `ambiguous_transport`, `backend_request_timeout`, or `incomplete_response`. `incomplete_response` means the *provider* stopped early; a stream the gateway itself ended (over quota, or quota no longer enforceable) is not a provider failure and carries no `failure` — read its error diagnostic instead |
| `retry_after_seconds` | Integer | Accepted bounded Retry-After cooldown, in seconds |
| `ttft_ms` | Integer | Time to the first non-empty response body chunk for the successful final attempt |
| `usage_source` | String | `reported` or `estimated` |
| `input_tokens` | Integer | Reported or bounded estimated input tokens for this attempt |
| `output_tokens` | Integer | Reported or bounded estimated output tokens for this attempt |
| `estimated_cost_usd` | String | Estimated attempt cost when pricing is configured |

The ledger never stores provider URLs, resolved addresses, Secret values, request/response bodies,
provider error text, prompts, completions, or tool arguments. For an ambiguous post-dispatch result,
Redis may conservatively retain the full input-plus-maximum-output reservation until its window
expires. That reservation protects the rate limit; it is distinct from the durable observed or
estimated usage recorded here and is not a claim that all reserved tokens were consumed.

### `stage_logs` fields

`stage_logs` is an array of stage groups; each group holds the per-plugin logs for one execution stage.

```json
"stage_logs": [
  {
    "stage": "request_filters",
    "logs": [
      { "name": "BasicAuth", "time_cost": 5, "log": ["Auth success"] },
      { "name": "Cors", "time_cost": 2, "log": ["CORS resp set"] }
    ]
  }
]
```

| Field | Type | Description |
|-------|------|-------------|
| `stage` | String | Execution stage. Examples: `request_filters`, `upstream_response_filters` |
| `logs[].name` | String | Plugin name |
| `logs[].time_cost` | Integer | Plugin execution time in **microseconds** |
| `logs[].cond_skip` | String | Why the plugin was skipped (`skip:<type>,<detail>` or `!run:<type>,<detail>`). Unchanged; omitted when the plugin ran. |
| `logs[].log` | Array\<String\> | Log lines emitted by the plugin. A composite condition scope (more than one atomic leaf) also pushes value-free descriptors for matched leaves, including their stable set location. Zero/one-leaf scopes suppress this redundant match-only signal. `cond_skip`, named-group attribution, final policy outcomes, and errors are unaffected. Body/dye evaluation stays silent. |
| `logs[].log_full` | Boolean | Present and `true` only when the plugin's log buffer overflowed and entries were dropped |

Common plugin log lines:

| Plugin | Log line | Meaning |
|--------|----------|---------|
| BasicAuth | `Auth success` / `Auth failed` / `Anonymous` | Authentication outcome |
| Cors | `CORS resp set` / `Preflight handled` / `Origin rejected` | CORS handling outcome |
| CSRF | `Token verified` / `Token set` / `Token mismatch` / `No token in header` | CSRF handling outcome |
| IPRestriction | `Allowed` / `Denied` | IP filtering outcome |
| Mock | `Mock returned` | Mock response was returned |

`log_full: true` means the plugin emitted more log data than the per-plugin buffer holds, so later lines were dropped. Treat it as a hint to trim the plugin's log output rather than as an error.

### `diagnostics` values

Diagnostics preserve encounter order and repeated entries. An `error` diagnostic records an
encountered failure, rejection, or degradation; it is not automatically an alert because a later
retry may recover. Alerting should be code-specific and may also consider the final status and
aggregate rate. Diagnostic codes have no numeric representation and do not imply an HTTP status.

| Kind | Code | Description |
|------|------|-------------|
| `error` | `RouteNotFound` | No matching route for the request |
| `error` | `XffHeaderTooLong` | `X-Forwarded-For` header exceeded the allowed length |
| `error` | `BackendRequestTimeoutRetry` | An upstream attempt hit its per-try timeout and was retried |
| `info` | `AcmeChallengeServed` | The gateway served an ACME HTTP-01 challenge locally |

## Extending the log: AccessLogExtern

The access log ships a fixed field set. To add route metadata, request/response
fields, or plugin-derived values, declare them with `AccessLogExtern` — the
single supported extension mechanism. It replaced five older, unrelated
carriers (route-annotation label projection, a JSON allowlist annotation, a
comma-separated ctx-key annotation, and an implicit CtxSet-to-annotation
coupling), all of which are gone with no aliases.

### It is a declaration, not a plugin

`AccessLogExtern` entries never execute as a request filter. They live at
`EdgionPlugins.spec.accessLogExtern` — a dedicated array, not one of the
request/response plugin stages — and are read only when the log line is
written. This is why `AccessLogExtern` fields still appear on responses that
never reach a plugin at all: a 404 with no matching route, a 421 SNI
mismatch, the 400 family, an ACME challenge response, or a CORS preflight
short-circuit.

```yaml
apiVersion: edgion.io/v1alpha1
kind: EdgionPlugins
metadata:
  name: audit-fields
  namespace: default
spec:
  accessLogExtern:
    - key: tenant        # field name inside the log entry's `ext` block
      from: ctx           # source type
      name: tenant_id     # key within that source
    - key: ua
      from: header
      name: user-agent
```

Each entry names exactly one source for exactly one field — there is no
template syntax for composing multiple values into one field. If you need a
composed value (e.g. `"{tenant}:{tier}"`), compute it once with the `CtxSet`
plugin and log the resulting ctx variable with `from: ctx`. Composing at the
`AccessLogExtern` layer was considered and rejected: a composed string has no
single provenance, so per-field masking could not apply to it correctly.

### Sources

| `from` | Reads | Masked? |
|---|---|---|
| `routeLabel` | A `metadata.labels` key on the matched HTTPRoute/GRPCRoute | Never |
| `routeAnnotation` | A `metadata.annotations` key on the matched HTTPRoute/GRPCRoute (`kubectl.kubernetes.io/*` and `edgion.io/*` keys are reported on `Accepted=False` at ingest and dropped when the gateway loads the resource — see the hard blocklist below) | Never |
| `header` | A request header (first value wins on repeats; non-UTF-8 values are absent) | Yes, unless whitelisted |
| `query` | The query string **as the client sent it**, frozen at request entry — unaffected by any later rewrite | Yes, unless whitelisted |
| `cookie` | A request cookie (`set-cookie`/response cookies are unreachable — see the hard blocklist) | Yes, unless whitelisted |
| `respHeader` | The response header actually written; absent when the upstream never responded (connect failure, timeout) | Yes, unless whitelisted |
| `ctx` | A plugin-set context variable (`CtxSet`, DSL `ctx.set`, a webhook `KeyGet` result, ...) — read after every producer at every stage | Yes, unless whitelisted |

A handful of names can never be named as a source, at any masking setting.
Naming one is reported on the `EdgionPlugins` resource's `Accepted=False`
status condition, and — independently of that report — the gateway drops the
field when it loads the resource, so it is never actually projected into a
log line:

| Source | Blocked names |
|---|---|
| `header` / `respHeader` | `authorization`, `proxy-authorization`, `cookie`, `set-cookie`, `x-api-key`, `api-key`, `apikey`, `x-auth-token`, `authentication`, `x-amz-security-token`, `www-authenticate`, `proxy-authenticate` |
| `ctx` | `jwt_claims`, `oidc_claims`, `jwe_payload` |
| `routeAnnotation` | anything under `kubectl.kubernetes.io/` or `edgion.io/` |

### Where fields land

All collected fields go into one nested `ext` object, one level deep, values
always rendered as strings:

```json
{"ts": 1752633600000, "request_info": {"host": "api.example.com", "path": "/users"}, "ext": {"tenant": "acme", "ua": "***"}}
```

`ext` is omitted from the line entirely when no field was collected for that
request.

### Attachment tiers

`AccessLogExtern` can be declared at up to three layers, applied in this
order — a later tier overwrites an earlier one's field on a key collision:

| Order | Tier | Where it's declared |
|---|---|---|
| 1 | GatewayClass-global | `EdgionGatewayConfig.spec.globalPluginsRef` |
| 2 | Gateway | `edgion.io/edgion-plugins` annotation on the `Gateway` |
| 3 | Route | An `ExtensionRef` filter on the HTTPRoute / GRPCRoute, pointing at an `EdgionPlugins` |

**Put fleet-wide audit fields on tier 1.** Tier 2 (the Gateway annotation) is
empty on any response that never matched a route — 404, 421, the 400 family,
ACME, gRPC `UNIMPLEMENTED` — because the Gateway a request would have used is
only known *after* route matching resolves it. Tier 1 resolves from the proxy
service directly and is present on every response, including those. This is
accepted behavior, not a bug: assigning a Gateway before matching would mean
guessing among the several Gateways a listener may serve.

**Give global-tier keys a distinct prefix** (e.g. `gw_tenant` instead of
`tenant`), because collision resolution is silent: if a route-level field
happens to share a key with a global field, the route's value wins and the
log gives no indication that a global field was overridden.

### Masking

`routeLabel` and `routeAnnotation` values are never masked — they are
operator-authored route configuration, not request data. Every other source
is masked unless the specific key is whitelisted in
`EdgionGatewayConfig.spec.accessLogExtern.unmaskedKeys` (keyed by source
type — `header`, `respHeader`, `query`, `cookie`, `ctx`). See
[EdgionGatewayConfig](../configuration/edgion-gateway-config.md) for the
whitelist schema and default entries.

A whitelist **miss still emits the field** — masked, not dropped — so an
operator sees `"ext":{"session":"***"}` and knows the value resolved but is
unwhitelisted, rather than staring at an absent field wondering whether the
source had no value, whether the field name was misspelled, or whether it was
a blocked name reported on `Accepted=False` and dropped when the gateway
loaded the resource.

Masked rendering follows the same global `access_log_sensitive.mode` switch
as the log's built-in sensitive fields (see [gateway configuration →
access_log_sensitive](../configuration/gateway-config.md#access_log_sensitive)).
**As of this release `access_log_sensitive.mode` defaults to `replace`**, so
out of the box, with no configuration required: `ext` fields render `***`
unless whitelisted, and `path`, `direct_peer_ip`, `remote_ip`,
`x-forwarded-for`, `sni`, and `request_uri` render `***` unconditionally —
the whitelist has no effect on these six built-in fields; they are masked
by `masked::serialize` regardless of `unmaskedKeys`.

**There is no configuration that gives readable `path` alongside a masked
`ext` cookie.** `mode: off` makes the whitelist inert (everything renders
clear, `ext` included); `mode: wrap` writes the raw value into the gateway's
own local log file/stdout, tagged `<E>value</E>` for a downstream pipeline to
strip before indexing — the raw value is still on local disk. Choose one of
these two trade-offs deliberately if you need readable paths; there is no
third option that keeps built-ins clear while masking `ext`.

### Known gaps (by design, not bugs)

- **`redirect_http` listeners (the plain HTTP→HTTPS redirect service) never
  emit `ext`.** They use a separate, minimal log schema with no `ext` field
  at all — there is no plugin chain on that path to attach a declaration to.
- **A second access-log line for gRPC-web/WebSocket connection establishment
  carries no `ext`** — only the request's final log line does. This is
  correct: the final line is the one the collector runs against.
- **The `X-Debug-Access-Log` response header is a non-terminal snapshot, not
  the terminal access-log record.** The `debug-access-log` plugin runs during
  the upstream-response-header stage. Its snapshot always omits `ext` and
  `waf`. Top-level `request_info.status` is always `0` because terminal
  `logging()` finalizes it later; independently,
  `backend_context.upstreams[].status` may already carry a status observed from
  an upstream attempt. Response-body and elapsed timing, final AI or streaming
  summaries, late diagnostics, and the current or later response-stage plugin
  logs may be absent or unfinished. With `access_log_sensitive.mode: replace`
  (the default), the header contains parseable JSON with sensitive fields
  replaced. `off` emits unmasked, unmarked raw values as an explicit debugging
  opt-out. `wrap` emits no header because wrapped values retain raw data;
  request processing continues, and `wrap mode:header suppressed` appears later
  in the terminal access log.

### Full example

```yaml
apiVersion: edgion.io/v1alpha1
kind: EdgionGatewayConfig
metadata:
  name: default-config
spec:
  accessLogExtern:
    unmaskedKeys:
      header: [user-agent, origin, content-type, content-length, accept, x-request-id]
      ctx: [tenant_id]
---
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: api-route
  namespace: production
spec:
  rules:
    - backendRefs: [{ name: api-service, port: 8080 }]
      filters:
        - type: ExtensionRef
          extensionRef:
            group: edgion.io
            kind: EdgionPlugins
            name: audit-fields
```

```json
{"ts": 1752633600000, "request_info": {"host": "api.example.com", "path": "/users", "status": 200},
 "ext": {"tenant": "acme", "ua": "Mozilla/5.0"}}
```

Both fields declared by the `audit-fields` resource [above](#it-is-a-declaration-not-a-plugin) render clear here: `ua` (`from: header, name: user-agent`) is whitelisted by `unmaskedKeys.header`, and `tenant` (`from: ctx, name: tenant_id`) is whitelisted by `unmaskedKeys.ctx`.

## Special fields

### Raw request URI

By default the log records the normalized `path`. To also capture the client-entry path and query before normalization or rewrites, enable `includeRawUri` under the top-level `access_log_options` section. For HTTP/1 absolute-form, this field omits the scheme and authority:

```yaml
access_log:
  enabled: true
  output:
    localFile:
      path: logs/edgion_access.log

access_log_options:
  includeRawUri: true
```

The raw URI then appears as the `request_uri` field. This is useful when debugging path rewrites or URL-encoding issues, but it increases line size, so enable it only while investigating.

## Common configuration scenarios

### 1. Quick local debugging

Enable the log to a local file with defaults — the simplest way to see what the gateway is doing during development:

```yaml
access_log:
  enabled: true
  output:
    localFile:
      path: logs/edgion_access.log
```

### 2. Production with daily rotation

Keep one file per day and retain a week of history:

```yaml
access_log:
  enabled: true
  output:
    localFile:
      path: logs/edgion_access.log
      queue_size: 80000
      rotation:
        strategy: daily
        max_files: 7
```

### 3. Investigating path rewrites

Capture both the normalized path and the raw client URI while debugging a rewrite problem:

```yaml
access_log:
  enabled: true
  output:
    localFile:
      path: logs/edgion_access.log

access_log_options:
  includeRawUri: true
```

## Log examples

### Basic HTTP request

```json
{
  "ts": 1767089146376,
  "request_info": {
    "trace_id": "4bf92f3577b34da6a3ce929d0e0e4736",
    "direct_peer_ip": "192.168.1.20",
    "client-port": 54882,
    "remote_ip": "192.168.1.20",
    "host": "test.example.com",
    "path": "/health",
    "status": 200
  },
  "match_info": { "rns": "production", "rn": "api-route", "rule_id": 0, "match_id": 0 },
  "backend_context": {
    "name": "api-service",
    "namespace": "production",
    "upstreams": [
      { "ip": "10.0.0.5", "port": 8080, "status": 200, "ct": 0, "ht": 0, "bt": 0, "et": 1 }
    ]
  }
}
```

### Request with a retry

```json
{
  "ts": 1767089146377,
  "request_info": {
    "host": "api.example.com",
    "path": "/api/data",
    "status": 200
  },
  "backend_context": {
    "name": "api-service",
    "namespace": "production",
    "upstreams": [
      { "ip": "10.0.0.5", "port": 8080, "status": 502, "ct": 1, "ht": 100, "et": 100, "err": ["Connection reset", "RetryOnStatus"] },
      { "ip": "10.0.0.6", "port": 8080, "status": 200, "ct": 2, "ht": 50, "bt": 50, "et": 52 }
    ]
  }
}
```

### Client disconnected before delivery

When the response cannot be delivered because the client already disconnected,
the request-level `diagnostics` array carries the error code `ClientClosed`. The status
is 499 when the disconnect itself ended the proxying; when the disconnect is
only discovered while writing the final error response, the record keeps the
original error status (e.g. 504, or 200 for gRPC trailers-only responses).
Filter on the `ClientClosed` diagnostic, not on `status == 499`. On
HTTP/1.1 the gateway notices the disconnect while an
upstream attempt is in flight and aborts within milliseconds, so the typical
record shows a single canceled attempt; a disconnect that lands in the retry
backoff/connect window can still leave multiple entries. Joining `ClientClosed`
with the `upstreams` array identifies and quantifies work spent on callers
that had already given up:

```json
{
  "request_info": { "status": 499 },
  "diagnostics": [{ "kind": "error", "code": "ClientClosed" }],
  "backend_context": {
    "upstreams": [
      { "ip": "10.0.0.5", "port": 8080, "status": 499, "ct": 0, "et": 512 }
    ]
  }
}
```

### Route not matched

```json
{
  "ts": 1767089149015,
  "request_info": {
    "host": "wrong-hostname.example.com",
    "path": "/test",
    "status": 404
  },
  "diagnostics": [{ "kind": "error", "code": "RouteNotFound" }],
  "backend_context": null
}
```

### gRPC request

```json
{
  "ts": 1767089148470,
  "request_info": {
    "host": "grpc.example.com",
    "path": "/test.TestService/SayHello",
    "status": 200,
    "discover_protocol": "grpc",
    "grpc_service": "test.TestService",
    "grpc_method": "SayHello"
  },
  "backend_context": {
    "name": "grpc-service",
    "namespace": "production",
    "upstreams": [
      { "ip": "10.0.0.21", "port": 8080, "status": 200, "ct": 0, "ht": 1, "bt": 1, "et": 1 }
    ]
  }
}
```

## Analyzing logs with jq

```bash
# All 4xx responses
cat logs/edgion_access.log | jq 'select(.request_info.status >= 400 and .request_info.status < 500)'

# Slowest requests by upstream header time
cat logs/edgion_access.log | jq -r '[.request_info.path, .backend_context.upstreams[0].ht] | @tsv' | sort -k2 -n

# Plugin executions over 100 microseconds
cat logs/edgion_access.log | jq '.stage_logs[]?.logs[]? | select(.time_cost > 100)'

# Trace one request end to end
cat logs/edgion_access.log | jq 'select(.request_info.trace_id == "4bf92f3577b34da6a3ce929d0e0e4736")'
```

Key fields to watch:

- `ht` (header time): backend time-to-first-byte; high values point to backend latency.
- `ct` (connect time): high values point to network or connection-pool issues.
- `et` (elapsed): total per-attempt time, including retries.
- `stage_logs[].time_cost`: plugin overhead in microseconds.
- `log_full: true`: a plugin's log buffer overflowed.

## FAQ

### Q1: I enabled the access log but the file is empty. Why?

**A**: Confirm `enabled: true` is set and `output.localFile.path` resolves to a writable location. Relative paths are resolved against `work_dir`, so check the actual working directory of the gateway process. Also confirm requests are actually reaching the gateway.

### Q2: Why are some fields missing from a log line?

**A**: Optional fields are omitted when empty rather than written as `null`. For example, `match_info` is absent when no route matched, `grpc_service` only appears for gRPC, and `sni` / `tls_id` only appear for TLS connections. This is expected and keeps lines compact.

### Q3: How do I correlate logs across a multi-hop request?

**A**: Filter `request_info.trace_id` for the standard trace. The default response header, when enabled, always equals that ID. Disabling the header leaves this log field present. Active custom IDs appear under `request_info.request_ids`; copied external values follow the log masking policy. See [RequestId](../../user-guide/http-route/filters/edgion-plugins/request-id.md).

### Q4: A request was rejected by a plugin — where do I look?

**A**: Check `stage_logs`. The rejecting plugin's `log` array typically carries the reason (for example `Auth failed` for BasicAuth or `Origin rejected` for Cors). When a plugin short-circuits the request, `backend_context` is usually `null`.

### Q5: How do I get more detailed plugin diagnostics for one route?

**A**: Use the debug access-log plugin to expose the non-terminal response-header-stage snapshot. It does not increase plugin verbosity. See [debug-access-log](../../user-guide/http-route/filters/edgion-plugins/debug-access-log.md).

## Get help

If you run into problems:

1. Check the FAQ section of this document
2. Check the gateway logs
3. Open an issue on GitHub

## Related documents

- [Metrics](./metrics.md)
- [debug-access-log Plugin](../../user-guide/http-route/filters/edgion-plugins/debug-access-log.md)
- [edgion-cli](../edgion-cli.md)

## OpenTelemetry Collector output

Declare an [OTLP LinkSys connector](../infrastructure/connectors.md#otlpgrpc-connector),
then reference it from the Gateway process configuration:

```yaml
access_log:
  enabled: true
  output:
    otlp:
      linkSysRef:
        namespace: observability
        name: collector
  fallback:
    localFile:
      path: logs/access-fallback.log
```

The same reference can be used by `tcp_log`, `tls_log` and `udp_log`. All share one
export pipeline. Missing or unhealthy references follow first-healthy fallback;
invalid reference syntax fails configuration. Connection fields belong only in
LinkSys. The exported JSON body, trace ID, status/severity and sensitive path
handling are unchanged. TCP health is not delivery acknowledgment.
