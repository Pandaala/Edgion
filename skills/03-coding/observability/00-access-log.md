# Access Log design and rules

> Core principle of access log: **a single entry can reconstruct the entire request**.
> Includes AccessLogEntry field design, PluginLog format rules, and a common-scenario quick reference.

## Core principles

### A single entry reconstructs the whole request

Each access log entry must contain enough information for an operator to locate the issue without flipping through other logs. Concerns include:

- **Routing match** (which route / backend was selected)
- **Key plugin results** (auth allow/deny, rate-limit hit, what was rewritten)
- **Upstream connection** (retry count, latency phases, final status code)
- **Request identity** (real IP, trace-id, sni)

### Record only key checkpoints, not the process

**Do not record**:
- Plugin-internal intermediate states ("starting auth check", "token parsed")
- Every header read or every conditional check
- Verbose duplicate debug steps (omit on the hot path; do not move them to `tracing::debug!`)

**Do record**:
- The plugin's **final decision** (allow / deny / what value was rewritten)
- **Exception paths** (connection failure, timeout, validation error reason summary)
- **Key parameters** (matched key, selected backend, retry count)

---

## GeoIP projection ownership

The optional `geo` block projects only already-resolved `remote` and `direct_peer`
slots through `RequestInfo::resolved_geo()`. Logging neither queries GeoIP nor
allocates optional geo state. Dimension strings borrow the request-retained
`GeoInfo` through the entry's lifetime; only the computed coordinate string is
owned and uses the canonical masked serializer. Serialize while the context is
alive: `to_json()` returns an owned string that can outlive the entry and context
for asynchronous sink or debug-header use. See the [GeoIP lazy-slot regression
boundary](../../04-review/observability/regression-guards/geoip-no-global-enabled-switch-access-log-lazy.md)
for ARCH-019 ownership evidence and verification limits.

## AccessLogEntry fields

`edgion-gateway/src/observe/access_log/entry.rs` — produced by `AccessLogEntry::from_context(ctx)` during the `logging()` phase of `pg_logging.rs`:

| Field | Meaning |
|------|------|
| `direct_peer_ip` | Physical downstream transport peer (`DirectPeerIp`) |
| `remote_ip` | Resolved client IP (`RemoteIp`) after the applicable PROXY/RealIp processing |
| `host` / `path` | Request host and the **normalized** current path (no query) |
| `request_uri` | **Optional** — client-entry **raw URI** (raw path + raw query); only emitted when `access_log_options.includeRawUri: true` |
| `request_info.trace_id` | canonical W3C ID string, 32 lowercase hexadecimal characters; present even when its wire header is disabled |
| `request_info.request_ids["<custom-header>"]` | active custom IDs in the request map; copied external values follow the log masking policy |
| `status` | Nginx/OpenResty `$status` analogue; final downstream response or terminal error status, with 0 meaning Nginx `000` unknown |
| `sni` | TLS SNI (HTTPS connections) |
| `tls_id` | TLS connection ID (used to correlate tls_access.log) |
| `discover_protocol` | Auto-detected protocol (grpc / websocket etc.) |
| `diagnostics` | One ordered request history of `{kind, code}` entries; `kind` is intrinsically `info` or `error` |
| `diagnostics_full` | Present and `true` only when the bounded request history dropped one or more tail entries |
| `match_info.sv` | sync_version — gRPC sync version, used to correlate control-plane logs (omitted when 0). See [../00-logging-and-tracing-ids.md](../00-logging-and-tracing-ids.md) |
| stage_logs | Per-phase plugin logs (see PluginLog below) |
| `waf` | Optional final native-WAF summary: selected source/priority/mode, fixed phase summaries, bounded loggable findings, first intervention, and first terminal error |
| upstreams | Upstream connection details (ip, port, ct/ht/bt/et); per-attempt retry reason in `err`: etype strings / `RetryOnStatus` / `ConnectionReuseRetry`. Client-disconnect waste is marked by the request diagnostic `{kind: "error", code: "ClientClosed"}` |
| `ext` | Operator-declared fields from `AccessLogExtern` (see below); omitted when nothing was collected |

### Native WAF block

The native WAF owns one request-wide top-level `waf` block instead of emitting
duplicate `PluginLog` entries under each stage. Only terminal `logging()` may
attach it; `AccessLogEntry::from_context` leaves it absent so connection-stage
and debug-header projections cannot expose rule identifiers or the active
protection posture.

The block is bounded to 16 retained loggable findings. The first intervention
and first terminal error are separate fixed-size facts, so they survive finding
truncation. `nolog,noauditlog` findings are excluded from both retained and
omitted loggable counts. Never add matched input, selected variable values,
headers, cookies, query/body bytes, rule messages, operator parameters, rule
source text, or formatted runtime errors. Closed error codes and numeric counts
belong here; diagnostic detail stays out of the data-plane access log.

### Final downstream/access-log status

`request_info.status` has one meaning: the Nginx/OpenResty `$status` analogue for the
finalized downstream HTTP response or terminal request error. It MUST never expose a
status merely observed from an upstream attempt, a response status that will be discarded
for retry, or a mutable pre-plugin/candidate response status.

It corresponds to access-log `$status`, NOT OpenResty's mutable `ngx.status`; the latter
is pre-header response-working state and belongs in separate non-serialized fields.

This is a protocol/access-log status, not proof that the peer application read the full
response. Finalize at terminal `logging()`, before metrics, OTLP metadata, or
`AccessLogEntry`, in this order:

1. The session's final downstream response header. Interim `100`/`102`/`103` headers are
   not final; `101 Switching Protocols` is terminal.
2. The explicit final downstream status already selected by a terminal response/outcome
   path when its header was not written.
3. Numeric `0`, representing Nginx's textual `000` unknown status.

Once a final header has committed, a later body timeout/disconnect does not rewrite the
status: a 200 header followed by a body failure remains 200 and gains an error diagnostic.
A selected terminal 502 whose error-page write later fails remains 502 plus
`ClientClosed`; it becomes 499 only when client closure itself was the request-ending
outcome before another final status was selected.

Mechanically, 499 may be selected only when a downstream transport error other than
`ReadTimedout` ends the request before final-header commit and neither a terminal error nor
a final downstream response already owns the outcome. A selected 200 whose header write
fails remains 200 + `ClientClosed`; a selected terminal 502 whose error-page write fails
remains 502 + `ClientClosed`.

A downstream `ReadTimedout` is the distinct stalled-request-body-upload case: select 408,
record `BodyReadTimeout`, do not record `ClientClosed`, and do not attach the timeout to the
selected backend attempt's `err` or local-origin outlier counter. Downstream `WriteTimedout`,
`ReadError`, `WriteError`, and `ConnectionClosed` remain client-closure outcomes at 499.
The 408 path is non-reusable and passes through the canonical HTTP/gRPC terminal response
dispatcher.

Each upstream attempt keeps its own observed/mapped status under
`backend_context.upstreams[].status`. Any response-stage safety check that needs the
pre-plugin upstream status uses a separate non-serialized snapshot; it must not borrow
`request_info.status` as scratch storage. Use a separate non-serialized final downstream
status while request processing is still active. For gRPC, `request_info.status` is the final
HTTP/2 status (normally `200`) and `request_info.grpc_status` is the independent final
gRPC application status selected or observed by the gateway. One must not be inferred
from the other.

Raw proxy failure codes are response-control inputs, not automatically HTTP access-log
status. Apply protocol mapping first: a generic gRPC upstream timeout uses raw 504 to
select `grpc_status: 4`, but the rendered downstream HTTP/access-log status is 200.
Likewise, diagnostic codes never map to or derive HTTP status. They serialize as stable
strings and have no numeric representation or implied HTTP prefix. The response path
records the explicit HTTP status it selected and diagnostics record reasons.

### Ordered request diagnostics

`diagnostics` is the single ordered diagnostic stream attached to the request. It replaces
request-scoped messages that would otherwise be scattered through a traditional proxy
error log, while each entry's intrinsic `kind` keeps normal noteworthy handling separate
from error analysis and alerting:

- `kind: "info"` records normal, noteworthy handling outcomes such as an ACME challenge
  served locally or an intentional plugin response replacement.
- `kind: "error"` records failures, rejects, degraded processing, failed upstream
  attempts, and other abnormal signals that an operator may query. An error entry means
  the condition occurred during the request; it does not by itself mean the final downstream
  status was a 4xx/5xx (a later retry may have succeeded).

Never alert merely because an error diagnostic exists. Alertability belongs to each
diagnostic code's contract and may also depend on the finalized access-log status and an
aggregate rate.
For example, `BackendRequestTimeoutRetry` followed by a successful final response is a
recovered attempt failure (normally aggregate for trend/SLO analysis, not a per-request
page), while an invariant marker such as `BodyRefusedNoSnapshot` is alertable at any
non-zero rate. Expected client/configuration outcomes may remain useful for queries without
being pager signals.

The stream is append-only for the request lifetime and preserves cross-kind occurrence
order. Do not deduplicate repeated entries, sort them, or reclassify their intrinsic kind
during final serialization. Repetition and order may describe the sequence of failed
attempts or degraded phases; per-attempt detail remains in `backend_context.upstreams`.
The collection uses inline storage for the common path, retains at most 32 entries, drops
only excess tail entries, and emits `diagnostics_full: true` when a drop occurs.

### When to add a new field

**Add only when all hold**:
1. The field is **frequently needed** during troubleshooting and cannot be inferred from other existing fields
2. The field is **request-level** (not plugin-internal intermediate state)
3. The field value **does not blow up storage** (avoid putting a large body or long token in)

**Good examples**:
- Add `grpc_service` / `grpc_method` (necessary for gRPC route troubleshooting)
- Add `client_cert_info` (knowing which cert was used after mTLS is enabled)

**Bad examples**:
- Putting the full request body into the access log
- Expanding every header into the access log

### Extending the access log: `AccessLogExtern`

`AccessLogExtern` is the single mechanism for projecting request/response
data, route metadata, or plugin-derived ctx vars into the access log. It
superseded the `edgion.io/log-label.<suffix>` prefix on HTTP/gRPC routes, a
unified JSON-array annotation, and a comma-separated ctx-key shorthand
annotation — all deleted — and fixed the undocumented "CtxSet writes
`ctx_map`, an annotation allowlist projects it" two-resource coupling. Full
design: `docs/superpowers/specs/2026-07-16-access-log-extern-plugin-design.md`.

**It is a declaration, not a plugin — it never executes in any stage.** It
lives at `EdgionPlugins.spec.accessLogExtern`, a dedicated array sibling to
`requestPlugins` (**not** one of the four stage arrays), and is read only when
the access log entry is serialized in `logging()`. This is load-bearing: a
`run_request` filter would never see 404 / 421 / the 400 family / ACME / gRPC
`UNIMPLEMENTED` (they short-circuit before any plugin runs), and would race
route-stage producers like `CtxSet` if attached globally. Resolving at
`logging()` fixes both at once.

```yaml
apiVersion: edgion.io/v1alpha1
kind: EdgionPlugins
metadata:
  name: audit-fields
spec:
  accessLogExtern:
    - key: tenant       # field name inside the entry's `ext` block
      from: ctx          # source type
      name: tenant_id    # key within the source
    - key: ua
      from: header
      name: user-agent
```

Each entry declares **exactly one field with exactly one source** — template
composition (`a|b|c` in one field) is deliberately unsupported, because a
composed string loses the provenance per-key masking needs. Compose in
`CtxSet` and log the resulting ctx var with `from: ctx` instead.

`from` is one of:

| `from` | Reads | Masking |
|---|---|---|
| `routeLabel` | Matched HTTPRoute/GRPCRoute `metadata.labels[name]` | Never masked |
| `routeAnnotation` | Matched HTTPRoute/GRPCRoute `metadata.annotations[name]` | Never masked |
| `header` | Request header (first value wins; non-UTF-8 is absent) | Masked unless whitelisted |
| `query` | The **entry-time raw query**, frozen at request entry — not the post-rewrite URI | Masked unless whitelisted |
| `cookie` | Request cookie (response `set-cookie` is unreachable — hard-blocklisted) | Masked unless whitelisted |
| `respHeader` | Written response header; absent when upstream never responded | Masked unless whitelisted |
| `ctx` | `ctx.ctx_map`, unconditionally after every producer at every stage | Masked unless whitelisted |

`routeLabel`/`routeAnnotation` are never masked because the value is
operator-authored route configuration, not request data — but `routeAnnotation`
additionally hard-blocks any `name` under the `kubectl.kubernetes.io/`
or `edgion.io/` prefixes (reported at ingest, dropped at load — see below),
because a route author is a tenant who did not write those keys
(`last-applied-configuration` holds the whole route spec JSON; `edgion.io/*`
holds plugin wiring).

**Hard blocklist** (reported on `Accepted=False` at ingest, dropped at load,
un-whitelistable, requires a code change to alter): `header`/`respHeader` —
`authorization`, `proxy-authorization`, `cookie`, `set-cookie`, `x-api-key`,
`api-key`, `apikey`, `x-auth-token`, `authentication`,
`x-amz-security-token`, `www-authenticate`, `proxy-authenticate`; `ctx` —
`jwt_claims`, `oidc_claims`, `jwe_payload` (`ACCESS_LOG_BLOCKED_CTX_KEYS`, the
same list as the old mechanism — but the *enforcement point* moved: the
controller's `preparse` only reports `Accepted=False` (lenient admission, the
resource is still cached and synced), so the actual drop now happens in
`compile_plugins`, which `retain()`s out any field `validation_error()`
rejects before the compiled spec is stored). A whitelist **miss** still emits
the key, masked — absence from `ext` means only "the source had no value",
never "was blocked".

**Output** lands in a nested `ext` block, one level deep, values always
strings:

```json
{"ts":1752633600000,"request_info":{"path":"/api/users"},"ext":{"tenant":"acme","ua":"***"}}
```

`ext` is omitted entirely when nothing was collected. This removes the old
duplicate-key hazard (`ctx_map` used to flatten at the entry's top level);
nesting means an `ext` key can never collide with a built-in field.

**Three attachment tiers**, later overwriting earlier on key collision
(**global → Gateway → route**):

| Tier | Carrier |
|---|---|
| GatewayClass-global | `EdgionGatewayConfig.spec.globalPluginsRef` |
| Gateway | `edgion.io/edgion-plugins` annotation on the Gateway |
| Route | HTTPRoute / GRPCRoute `ExtensionRef` filter |

**Two things to know before you configure this — both are easy to learn the
hard way**:

1. **`access_log_sensitive.mode` now defaults to `replace`.** `ext` rendering
   reuses the same global switch as the nine built-in masked fields (`path`,
   `direct_peer_ip`, `remote_ip`, `x-forwarded-for`, `sni`, `request_uri`, and
   the client-cert fields), 46 `sensitive()` plugin-log sites, DSL
   `log_warn`/`log_error` args, and ssl.log cert fields. All of it renders
   `***` out of the box now, not just `ext`.
2. **There is no configuration giving readable `path` + masked `ext` cookie.**
   `off` makes the EGC whitelist inert (everything clear); `wrap` puts the raw
   value in the log file, tagged `<E>value</E>` for the downstream pipeline to
   drop — the gateway's local file/stdout still has it in clear. This is a
   deliberate design trade (one uniform switch, not per-category rendering
   policy), not an oversight.

**Naming convention for global fields.** Because collision resolves
route-beats-Gateway-beats-global with no enforcement, a route can silently
shadow a global audit field and the log gives no sign it happened. Prefix
global-tier keys distinctively (e.g. `gw_tenant`) so a shadowed field is at
least visually distinguishable from an unshadowed one. This is a documented
risk, not something the controller defends against — enforcing it would need
a reverse dependency from global config to every route.

**Accepted limitations (not bugs, do not "fix" these):**

- **The Gateway tier (Tier 2) is empty on pre-routing short-circuits** — 404,
  421, the 400 family, ACME, gRPC `UNIMPLEMENTED`. `ctx.gateway_info` is
  *produced by* route matching, so on a 404 no Gateway is attributable at all.
  Put audit fields that must appear on every path — including 404 — on
  **Tier 1** (`globalPluginsRef`), which resolves from the proxy service and
  is present unconditionally.
- **`redirect_http` listeners never emit `ext`.** It is a separate `ProxyHttp`
  impl with its own `RedirectLogEntry` schema, which has no `ext` field at all.
- **Two other call sites emit no `ext`, both correctly:**
  `pg_connected_to_upstream.rs` writes a *second* access-log line for
  grpc-web/websocket at connection establishment (the request's final
  `logging()` line still carries `ext`); and `DebugAccessLogToHeader` exposes
  a non-terminal upstream-response-header-stage snapshot in the
  `X-Debug-Access-Log` **response header**, not the terminal access-log record.
  That snapshot always omits both `ext` and `waf`. Its top-level
  `request_info.status` is always `0` because terminal `logging()` finalizes it
  later; independently, `backend_context.upstreams[].status` may already carry
  a status observed from an upstream attempt. Response-body and elapsed timing,
  final AI or streaming summaries, late diagnostics, and the current or later
  response-stage plugin logs may be absent or unfinished. With the default
  `access_log_sensitive.mode: replace`, the header contains parseable JSON with
  sensitive fields replaced; `off` exposes unmasked, unmarked raw values only
  as an explicit debugging opt-out. `wrap` exposes no header because wrapped
  values retain raw data; processing continues, and the ordinary PluginLog
  decision `wrap mode:header suppressed` appears later in the terminal access
  log.

**EGC masking whitelist** — `EdgionGatewayConfig.spec.accessLogExtern.unmaskedKeys`,
keyed by source type (`header`/`respHeader`/`query`/`cookie`/`ctx`). Full
schema in [`../../02-features/02-config/02-edgion-gateway-config.md`](../../02-features/02-config/02-edgion-gateway-config.md).

---

## Plugin Log (PluginLog) rules

Each plugin writes a compact execution summary to the access log via `log: &mut PluginLog`.

### Format

```
<result-prefix>[:<key parameters>]
```

- **Result prefixes are uniformly defined**:

| Prefix | Meaning | Write API | Example |
|------|------|---------|------|
| `OK` | Allowed normally | `plugin_log.push("OK ...")` | `OK u=jack` |
| `Deny` | Business-level reject (auth fail, rate limit, etc.) | `plugin_log.push("Deny ...")` | `Deny no-key` |
| `Skip` | Condition not met, skipped | `plugin_log.push("Skip")` or `set_cond_skip` | `Skip` |
| `Rate` | Rate-limit triggered | `plugin_log.push("Rate ...")` | `Rate 429` |
| **`err: `** | **Exception / config degradation** — replaces data-plane `tracing::warn!` | **`plugin_log.push_err("...")`** (see below) | `err: token refresh fail` |

- Key parameters use the `k=v` format; multiple parameters are space-separated
- New code always uses `push_err` for error entries

### Condition-derived signals (`cond_skip` and descriptors)

Two independent signals. Do not collapse them.

| Signal | Where | When | Meaning |
|--------|--------|------|---------|
| `cond_skip` | `PluginLog.cond_skip` (`set_cond_skip`) | Plugin skipped by `PluginConditions` | Why it was skipped: `skip:<type>,<detail>` or `!run:<type>,<detail>`. Format and producer are unchanged (D-23). |
| Compiled descriptors | plugin `log` array (`PluginLog::push`) | A condition **matches** within a multi-leaf request/response decision scope | `<path>:<type>[:<accessor>]`. Value-free. Non-match logs nothing. Body/dye stay silent. |

Within a `ConditionSet`, `anyOf` records only the first successful branch;
`allOf` records each success in order and stops on the first failure. A logical
decision scope with zero or one atomic leaf emits no match-only descriptor;
`cond_skip`, the final policy/plugin decision, named-group attribution, and
errors remain visible. The scope is combined `skip + run` for plugin entry
conditions, the whole expression for RequestRestriction, and one typed
rule/candidate supplemental expression for RequestAccessPolicy. Leaf counts
and the emission flag are computed when the runtime snapshot is built. See
[`docs/en/user-guide/http-route/filters/edgion-plugins/conditions.md`](../../../docs/en/user-guide/http-route/filters/edgion-plugins/conditions.md).

### Error-entry API constraint: must use `push_err`

> **Authoritative source**: this section is also part of iron rule 1 in `skills/03-coding/01-log-safety.md`. This section explains "what it looks like in the access log"; log-safety.md explains "why the constraint and the safety rule".

**API**:

```rust
// Defined at: edgion-gateway/src/plugins/runtime/log.rs
impl PluginLog {
    pub fn push_err(&mut self, kind: &'static str) -> bool;   // Routes through the dual-tier buffer
}
```

**Call shape**:

```rust
// Correct — kind is a literal, no plugin prefix (plugin identity is in PluginLog.name)
plugin_log.push_err("token refresh fail");    // → "err: token refresh fail"
plugin_log.push_err("jwks fetch timeout");    // → "err: jwks fetch timeout"
plugin_log.push_err("cfg invalid");           // → "err: cfg invalid"

// Bounded enum errors — manually map to a static label
match jwt_err.kind() {
    ErrorKind::InvalidSignature => plugin_log.push_err("jwt sig invalid"),
    ErrorKind::ExpiredSignature => plugin_log.push_err("jwt expired"),
    _                           => plugin_log.push_err("jwt other"),
};

// Won't compile — the &'static str signature blocks all dynamic strings
plugin_log.push_err(&format!("oidc: {}", e));
plugin_log.push_err(&err.to_string());
plugin_log.push_err(dynamic_kind.as_str());

// Even if it compiled, bypassing push_err via push to write errors is forbidden (leak risk)
plugin_log.push(&format!("Config error: {}", err));       // Display may contain token / claim
plugin_log.push(&format!("Auth failed for user {}", name)); // PII into PluginLog
```

**`kind` literal conventions**:

- Lowercase words + **spaces** as separators (not underscores), 15-40 bytes
- Write only "what happened" (the event name); **no plugin prefix** (`PluginLog.name` already carries it)
- No dynamic values (token / claim / URL / header / error Display)
- A two-segment `<event>:<action>` colon syntax is allowed (both segments are full words). See "PluginLog entry wording rules" §1 rule 4 below

**When to use `push_err` vs continuing to write to `ctx.err_log`**: `push_err` does **classification**; `ctx.err_log` keeps **the masked details that this error must preserve** (e.g., the backend address). Record each error in only one place — never both.

**Safety background**: full reasoning for the `&'static str` constraint, `Box::leak` escape notes, and safety checklist → see "PluginLog error entries: must use `push_err`" in `skills/03-coding/01-log-safety.md`.

### Good PluginLog examples

```rust
// KeyAuth — auth passed (group attribution always present after secret_groups pilot)
log.push_fmt("OK group={} user {}", la!["partner-a", sensitive(&"jack")]);
// Note: failure branches (`Deny bad key`, `Deny no key`) deliberately omit `group=`
// to avoid leaking multi-tenant topology to unauthenticated callers.

// KeyAuth — key not found
log.push("Deny no-key");

// RateLimitLocal — limit triggered
log.push(&format!("Rate 429 k=user:{}", user_id));

// RateLimitLocal — not triggered
log.push("OK");

// BasicAuth — auth failed
log.push("Deny bad-cred");

// Redirect — performed a redirect
log.push(&format!("-> {} {}", code, location));

// DirectEndpoint — endpoint forced
log.push(&format!("Direct {}:{}", ip, port));

// OIDC — token refresh failure (error classification, replacing tracing::warn!)
log.push_err("token refresh fail");

// RegionRoute — failoverTo config invalid (plugin degradation, region skips redirect)
log.push_err("failover to invalid");
```

### Bad PluginLog examples

```rust
// Too verbose, fills the buffer
log.push("Starting JWT authentication process for incoming request");
log.push("Successfully parsed and validated JWT token with RSA256 algorithm");

// Internal detail, not a final decision
log.push("Checking rate limit window...");
log.push("Window size = 60s");

// Sensitive internal detail: do not record on the request hot path
log.push(&format!("token_claims = {:?}", claims));

// Error path via push + format! — leak risk + bypasses the push_err constraint
log.push(&format!("OIDC token refresh failed: {}", jwt_err));
log.push(&format!("Auth failed for user {}", username));
```

### Optional `alias` field

Each `PluginLog` may carry an optional operator-defined `alias` (echoed verbatim from the corresponding `EdgionPlugins` entry's `alias` field). When present, it serializes as a sibling field to `name`:

```json
{ "name": "KeyAuth", "alias": "auth-internal", "time_cost": 42, "log": ["OK u=jack"] }
```

Use it to distinguish multiple entries of the same plugin type. The alias is stored independently of the LogBuffer used by `push` / `push_err`, so adding an alias does **not** reduce buffer space available for plugin log entries. Allowed character set: `[a-zA-Z0-9._-]`, length 1–32.

### Buffer limit

`PluginLog` has a two-tier buffer, but the tiers are an **internal** detail —
they serialize as **one merged `"log"` array** in push order:

- **Tier 1** — a fixed 128-byte stack buffer (zero heap allocation). Most
  plugins only ever touch this tier. Keep each entry within **20~40 bytes** so
  several plugins can write together.
- **Tier 2** — a 512-byte heap buffer, created lazily only when tier 1
  overflows. Overflow entries are appended after the tier-1 entries; once tier 2
  is in use every subsequent entry goes there, so global push order is preserved.

The JSON output is always a single `"log"` array — there is no separate `ulog`
field and no in-band overflow marker. When even tier 2 is full, further entries
are dropped and `log_full: true` is set. Do not use tier 2 as a request-level
debug stream; omit intermediate detail or add a deliberately bounded final signal.

---

## Common-scenario quick reference

### Scenario 1: plugin made an auth decision
```rust
log.push("OK u=alice");          // allowed
log.push("Deny missing-token");  // rejected, with reason
```
No extra metric is needed; the result is reflected in the response status and
`edgion_backend_requests_total`.

### Scenario 2: plugin rewrote the request
```rust
log.push(&format!("Rewrite -> {}", new_path));
```

### Scenario 4: upstream connection failure
The upstream error message flows into the access log via the `UpstreamInfo.err` field; the plugin does not need to record it separately.

### Scenario 5: tracing log or access log?

| Information type | Destination |
|---------|------|
| Final request decision | `PluginLog` → access log |
| Intermediate request steps | Omit; reconstruct from bounded final access-log fields |
| Request exception | `diagnostics`, `PluginLog`, or an existing bounded metric |
| Cold/config/background exception | Structured `tracing::warn!()` / `tracing::error!()` |
| Performance data | `Gauge` / `Counter` in `metrics.rs` → Prometheus |

---

---

## Path / URI variable semantics (after path normalization)

### Variable cheat sheet

| Variable | View | Meaning | Affected by rewrite |
|------|------|------|----------------|
| `$path` | current | Normalized current path (post-rewrite) | Yes |
| `$uri` | current | Normalized current path + current query | Yes |
| `$query` | current | Current query (not normalized) | Yes |
| `$request_path` | entry (frozen) | Raw client-entry path | No |
| `$request_uri` | entry (frozen) | Raw client-entry path + raw query | No |

### Behavior changes (migration note)

- **`$path` / `$uri`** semantics switched from "raw current" to "normalized current". Behavior is unchanged for already-normalized requests; for requests with encoded chars / double slashes / dot-segments, values differ
- **`$request_path` / `$request_uri`** are newly added variables that reflect the raw client form; frozen and unaffected by rewrite

### Behavior after rewrite

After plugins like proxy_rewrite / url_rewrite modify the path:

- `$uri` reflects the **new path** (the ctx cache is auto-refreshed via `session.set_upstream_uri`)
- `$request_uri` still reflects the **raw entry**

Example (request `GET /%61dmin/data`, proxy_rewrite changes path to `/backend/data`):

```
$path         = /backend/data   (new path after rewrite, normalized)
$uri          = /backend/data   (same, no query)
$request_path = /%61dmin/data   (raw entry, frozen)
$request_uri  = /%61dmin/data   (raw entry + query, frozen)
```

### (v4) Relationship between `$uri` and the path actually received by upstream

Since v4 Edgion forwards the normalized path to upstream by default (aligned with Envoy / Kong). In most cases `$uri` reflects what upstream received. But **two raw-forward opt-out scenarios** make `$uri` (normalized) diverge from the path received by upstream:

| Scenario | Trigger | `$uri` value | Upstream receives |
|---|---|---|---|
| Default | No annotation, no region_route, `forward_normalized_path=true` | normalized | normalized (consistent) |
| HTTPRoute annotation force raw | `edgion.io/forward-raw-path: "true"` | normalized | **raw** (divergent!) |
| `region_route` hits | Region cross-rule hit, no annotation override | normalized | **raw** (divergent!) |
| Global fallback | `forward_normalized_path = false` | normalized | **raw** (divergent!) |

**Implications for audit/observability**:
- `$uri` is always the normalized view, with controlled cardinality, suitable for Prometheus labels / aggregate queries
- For "what the client actually sent", use `$request_uri` (entry raw view, always frozen)
- For "what upstream actually received" — **neither directly equals it**. You need:
  1. Check whether the route carries the `forward-raw-path: "true"` annotation (the access log can include `route_namespace` / `route_name` tags so you can look it up in K8s)
  2. Or record the path actually received in the upstream application
  3. Or refer to spec §14.5 "mirror vs upstream view divergence" notes

For the detailed decision matrix see spec `docs/superpowers/specs/2026-04-26-uri-normalization-design.md` §14.

### Query view asymmetry (known gap)

`$request_query` (entry query) **does not exist**. If you need to audit the entry query, split it from `$request_uri`.

- Need "the full URI actually sent by the client" → use `$request_uri`
- Need "the URI forwarded to upstream after rewrite" → use `$uri`
- Need the entry query alone → split it from `$request_uri`

### Fields in JSON access log

The JSON access log emits only the **normalized** path by default; no raw-form fields, no query:

```json
{ "ts": ..., "path": "/api/users/data", "host": "...", "status": 200, ... }
```

When "the full URI sent by the client" (with query) is needed, enable the global option:

```yaml
access_log_options:
  includeRawUri: true
```

An additional `request_uri` field is emitted, valued as entry raw path + entry raw query:

```json
{ "ts": ..., "path": "/api/users/data",
  "request_uri": "/api/users;v=1/data?from=foo",
  "host": "...", "status": 200, ... }
```

Field semantics:

| Field | View | Default emitted |
|------|------|--------------|
| `path` | normalized current (no query) | Yes |
| `request_uri` | raw entry path + raw entry query | No (hidden by default; needs the toggle) |

---

## Key Files

- `edgion-gateway/src/ctx.rs` — `EdgionHttpContext`, `MatchInfo` (with `sv` field), `MatchedInfo` (with `sv` field)
- `edgion-gateway/src/observe/access_log/entry.rs` — `AccessLogEntry`
- `edgion-gateway/src/observe/access_log/logger.rs` — `AccessLogger`
- `edgion-gateway/src/observe/logs/logger_factory.rs` — `create_async_logger()`
- `edgion-gateway/src/plugins/runtime/log.rs` — `PluginLog`, `LogBuffer` (128-byte SmallVec, tier 1) + `ULogBuffer` (512-byte overflow, tier 2)

---

## PluginLog entry wording rules

### 1. Entry literal style (shared by push and push_err)

1. All-lowercase English words separated by spaces; each entry 15-40 bytes, ≤ 4 words (including the action verb)
2. No abbreviations. Whitelist (kept as-is): ip, tls, http, https, tcp, udp, sni, id, url,
   dns, jwt, jwks, cors, csrf
3. Numeric literals allowed: rate 429, retry 2, size 1024
4. Event+action joined by colon (both sides full words, no whitespace squeezing):
   `"key miss:allow"`, `"jwks refresh:stale"`, `"upstream select:failover"`
   Single-segment info has no colon:
   `"upstream b selected"`, `"OK user jack"`
5. Reserved prefix verbs (do not rename): OK / Deny / Rate / Allow / Skip
6. **Sensitive dynamic values are forbidden**. The strings of push / push_err must not contain: token, claim values,
   raw header value, cookie, email, secrets (API key, password, session id),
   request/response body.
   **Non-sensitive dynamic values allowed** (push only; push_err arguments are already &'static str at compile time):
   - `ip`, `port`, `path`, http status code, `method`
   - Numeric (count, duration_ms, size, retry_n)
   - User identity (username, user_id) — already in the access log; consistency takes priority
   - Bounded tags (region name, upstream group name — code-level static labels)
   **Error paths always go through push_err** (&'static str); never use `push(&format!("...{}", err))`
   to splice error info — Display may leak token/claim.
7. Plugin name is not written into the entry (already in the PluginLog.name field). Fix example:
   "rl:nok:a" → "key miss:allow"
8. **Trailing `"; "` is removed entirely**: `push` entries never carry a trailing `"; "`. `push_err` output is `"err: <kind>"` (no suffix). Suggested examples: `log.push("OK user jack")`, `log.push(&format!("Direct {}:{}", ip, port))`.

### 2. push vs push_err

- push: normal decisions, branch hits, business-level allow/deny, rate-limit results, degraded-but-still-proceeding
  Examples: `"OK user jack"` / `"Deny bad credential"` / `"Rate 429"` / `"key miss:allow"`
- push_err: per-request exceptions (config not readable, remote call failure, decoding failure, internal
  unreachable branch)
  Examples: `"token refresh fail"` / `"cache load fail"` / `"jwks fetch:timeout"`
  (push_err's kind has no prefix/suffix; the runtime auto-completes `"err: <kind>"`)
kind takes the form `"<action> fail"` or `"<action>:<reason>"`. Both can be called multiple times within the same plugin;
the log is finally serialized as an array:
`log: ["OK user jack", "err: jwks fetch:timeout", "OK fallback"]`.

### 3. Key-checkpoint selection

Add when (removing the entry would make the request flow unreadable in the access log):
- Which upstream / endpoint / region / failover was selected
- Which auth algorithm or key source was selected
- Which key/bucket the rate limit hit, and the result (allow/throttle/deny)
- Cache hit/miss, whether to fall back
- Which rule/header/pattern matched
- Every branch that can enter PluginRunningResult::ErrTerminateRequest (must push_err to leave a trace)

Do not add:
- Function entry/exit, header parsing, config read
- Each iteration inside a loop
- Wrapper-layer run_if/skip_if (already handled by conditional_filter.rs via set_cond_skip)
- Restating an exception with another push("failed") when push_err already exists

### 4. tracing boundary (hard rule)

Within the per-request call chain (filter-entry synchronous call stack):
- Errors → push_err
- Decisions → push
- tracing::{error,warn,info,debug,trace}! is forbidden

Locations where tracing::*! may stay: plugin constructors / new() / config load / background tasks
(JWKS rotation, cache refresh timer, Redis reconnect) / module initialization.

---

## Plugin access-log token contract

When integration tests assert on access-log substrings of a plugin and the format is non-trivial, mirror those substrings into a sibling `log_consts.rs` module with a `#[cfg(test)] include_str!`-based guard. Reference implementation: `edgion-gateway/src/plugins/http/region_route/log_consts.rs`.

**Apply this when:**
- An integration test broke because the plugin's log format changed under the assertion (drift incident).
- Adding a new plugin whose access log is part of the integration contract.

**Do NOT pre-apply** to stable plugins that have never drifted — the convention is reactive.

**Rule for what becomes a token:** const-ize a substring iff some integration test asserts it today, AND the substring is plugin-contract (not fixture-controlled and not a single high-collision character like `→` or `=`). The plugin source keeps literal fmt strings; the const layer is purely a test-side contract anchored by an `include_str!` source-grep test.

Spec: `docs/superpowers/specs/2026-06-16-plugin-log-tokens-design.md`.

### Request identity ownership

`RequestInfo.trace_id: RequestTraceId` owns the internal W3C context and bounded vector of correlation IDs. Serialize the W3C ID as the `request_info.trace_id` string and active custom names under `request_info.request_ids`. Disabling the default header does not remove the standard log identity. Custom destinations cannot reuse the reserved wire header `x-trace-id`. Preserve origin metadata: copied custom values require log masking without altering wire propagation. Suppressed custom IDs are omitted. A business ID or W3C trace ID is not a unique per-request storage key; multiple requests may share it. See [lifecycle contract](../../04-review/observability/intentional-design/request-trace-id-lifecycle.md).

## OTLP connection and identity boundary

OTLP senders retain only a validated LinkSys key. The shared persistent runner
publishes a small OtlpLinkClient generation that owns its connection,
credentials, TLS, exporter/batch state, and health. Keep record
projection (LogMeta, severity, trace ID, body and sensitive url.path) unchanged
when touching transport. Do not add per-request tracing or export acknowledgments
to the first-healthy sender contract. Health labels use bounded namespace/name,
never an endpoint or raw process-config reference. Native transport dependency
logs are protected by shared bootstrap independently of RUST_LOG.
