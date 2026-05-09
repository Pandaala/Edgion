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
- Verbose duplicate debug info (those belong to `tracing::debug!()`)

**Do record**:
- The plugin's **final decision** (allow / deny / what value was rewritten)
- **Exception paths** (connection failure, timeout, validation error reason summary)
- **Key parameters** (matched key, selected backend, retry count)

---

## AccessLogEntry fields

`src/core/gateway/observe/access_log/entry.rs` — produced by `AccessLogEntry::from_context(ctx)` during the `logging()` phase of `pg_logging.rs`:

| Field | Meaning |
|------|------|
| `client-addr` | Direct TCP IP |
| `remote-addr` | Real client IP (after RealIp plugin processing) |
| `host` / `path` | Request host and the **normalized** current path (no query) |
| `request_uri` | **Optional** — client-entry **raw URI** (raw path + raw query); only emitted when `[access_log_options] include_raw_uri = true` |
| `x-trace-id` | trace id (passed from client or auto-generated UUID) |
| `status` | Final response status code |
| `sni` | TLS SNI (HTTPS connections) |
| `tls_id` | TLS connection ID (used to correlate tls_access.log) |
| `discover_protocol` | Auto-detected protocol (grpc / websocket etc.) |
| `info` | Gateway-internal normal handling events (e.g., ACME local hit); not an error |
| `errors` | Gateway-internal exception/reject/failure reasons |
| `match_info.sv` | sync_version — gRPC sync version, used to correlate control-plane logs (omitted when 0). See [../00-logging-and-tracing-ids.md](../00-logging-and-tracing-ids.md) |
| stage_logs | Per-phase plugin logs (see PluginLog below) |
| upstreams | Upstream connection details (ip, port, ct/ht/bt/et, retry) |

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

---

## Plugin Log (PluginLog) rules

Each plugin writes a compact execution summary to the access log via `log: &mut PluginLog`.

### Format

```
<result-prefix>[:<key parameters>]; 
```

- Ends with `; ` (eases grep / concatenation)
- **Result prefixes are uniformly defined**:

| Prefix | Meaning | Write API | Example |
|------|------|---------|------|
| `OK` | Allowed normally | `plugin_log.push("OK ...; ")` | `OK u=jack; ` |
| `Deny` | Business-level reject (auth fail, rate limit, etc.) | `plugin_log.push("Deny ...; ")` | `Deny no-key; ` |
| `Skip` | Condition not met, skipped | `plugin_log.push("Skip; ")` or `set_cond_skip` | `Skip; ` |
| `Rate` | Rate-limit triggered | `plugin_log.push("Rate ...; ")` | `Rate 429; ` |
| **`err: `** | **Exception / config degradation** — replaces data-plane `tracing::warn!`; separated by space-colon | **`plugin_log.push_err("...")`** (see below) | `err: token refresh fail; ` |

- Key parameters use the `k=v` format; multiple parameters are space-separated
- Some legacy code uses no-space old forms `"err:failoverTo invalid; "` / `"rl:err:cfg; "` (6 sites total in region_route + rate_limit). These are **legacy** and are migrated to `push_err` by the corresponding wave tickets; new code always uses `push_err`

### Error-entry API constraint: must use `push_err`

> **Authoritative source**: this section is also part of iron rule 1 in `skills/03-coding/01-log-safety.md`. This section explains "what it looks like in the access log"; log-safety.md explains "why the constraint and the safety rule".

**API**:

```rust
// Defined at: src/core/gateway/plugins/runtime/log.rs
impl PluginLog {
    pub fn push_err(&mut self, kind: &'static str) -> bool;   // Fixed buffer
    pub fn u_push_err(&mut self, kind: &'static str) -> bool; // Debug buffer
}
```

**Call shape**:

```rust
// Correct — kind is a literal, no plugin prefix (plugin identity is in PluginLog.name)
plugin_log.push_err("token refresh fail");    // → "err: token refresh fail; "
plugin_log.push_err("jwks fetch timeout");    // → "err: jwks fetch timeout; "
plugin_log.push_err("cfg invalid");           // → "err: cfg invalid; "

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
plugin_log.push(&format!("Config error: {}; ", err));       // Display may contain token / claim
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
// KeyAuth — auth passed
log.push("OK u=jack; ");

// KeyAuth — key not found
log.push("Deny no-key; ");

// RateLimit — limit triggered
log.push(&format!("Rate 429 k=user:{}; ", user_id));

// RateLimit — not triggered
log.push("OK; ");

// BasicAuth — auth failed
log.push("Deny bad-cred; ");

// Redirect — performed a redirect
log.push(&format!("-> {} {}; ", code, location));

// DirectEndpoint — endpoint forced
log.push(&format!("Direct {}:{}; ", ip, port));

// OIDC — token refresh failure (error classification, replacing tracing::warn!)
log.push_err("token refresh fail");

// RegionRoute — failoverTo config invalid (plugin degradation, region skips redirect)
//   New code:
log.push_err("failover to invalid");
//   Legacy form (not yet migrated in `region_route/plugin.rs`):
//   log.push("err:failoverTo invalid; ");
```

### Bad PluginLog examples

```rust
// Too verbose, fills the buffer
log.push("Starting JWT authentication process for incoming request");
log.push("Successfully parsed and validated JWT token with RSA256 algorithm");

// Internal detail, not a final decision
log.push("Checking rate limit window...");
log.push("Window size = 60s");

// Belongs to tracing::debug!
log.push(&format!("token_claims = {:?}", claims));

// Error path via push + format! — leak risk + bypasses the push_err constraint
log.push(&format!("OIDC token refresh failed: {}", jwt_err));
log.push(&format!("Auth failed for user {}", username));
```

### Optional `alias` field

Each `PluginLog` may carry an optional operator-defined `alias` (echoed verbatim from the corresponding `EdgionPlugins` entry's `alias` field). When present, it serializes as a sibling field to `name`:

```json
{ "name": "KeyAuth", "alias": "auth-internal", "time_cost": 42, "log": "OK u=jack; " }
```

Use it to distinguish multiple entries of the same plugin type. The alias is stored independently of the 100-byte LogBuffer used by `push` / `push_err`, so adding an alias does **not** reduce buffer space available for plugin log entries. Allowed character set: `[a-zA-Z0-9._-]`, length 1–32.

### Buffer limit

`PluginLog` has a fixed-size buffer (default 100 bytes). Each entry should stay within **20~40 bytes** to leave room for multiple plugins to write together. Anything beyond the buffer is truncated.

For detailed debug info, use `tracing::debug!`.

---

## Common-scenario quick reference

### Scenario 1: plugin made an auth decision
```rust
log.push("OK u=alice; ");          // allowed
log.push("Deny missing-token; ");  // rejected, with reason
```
No metric needed (the auth result already shows up in the status of `requests_failed` and `backend_requests_total`).

### Scenario 2: plugin rewrote the request
```rust
log.push(&format!("Rewrite -> {}; ", new_path));
```

### Scenario 4: upstream connection failure
The upstream error message flows into the access log via the `UpstreamInfo.err` field; the plugin does not need to record it separately.

### Scenario 5: tracing log or access log?

| Information type | Destination |
|---------|------|
| Final request decision | `PluginLog` → access log |
| Intermediate steps for troubleshooting | `tracing::debug!()` → system log |
| Exception/alert | `tracing::warn!()` / `tracing::error!()` → system log |
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

```toml
[access_log_options]
include_raw_uri = true
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

- `src/types/ctx.rs` — `EdgionHttpContext`, `MatchInfo` (with `sv` field), `MatchedInfo` (with `sv` field)
- `src/core/gateway/observe/access_log/entry.rs` — `AccessLogEntry`
- `src/core/gateway/observe/access_log/logger.rs` — `AccessLogger`
- `src/core/gateway/observe/logs/logger_factory.rs` — `create_async_logger()`
- `src/core/gateway/plugins/runtime/log.rs` — `PluginLog`, `LogBuffer` (100-byte SmallVec)

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
8. **Trailing "; " is not required**: push entries do not need a manual trailing "; " (during JSON-array serialization each entry is already a separate element).
   The runtime of push_err still automatically prepends `"err: <kind>; "` (built-in behavior of buffer.rs); that part is unchanged.
   Suggested examples: `log.push("OK user jack")`, `log.push(&format!("Direct {}:{}", ip, port))`.
   (Legacy "; " entries can be kept for now; cleanup will follow in a later review.)

### 2. push vs push_err

- push: normal decisions, branch hits, business-level allow/deny, rate-limit results, degraded-but-still-proceeding
  Examples: `"OK user jack"` / `"Deny bad credential"` / `"Rate 429"` / `"key miss:allow"`
- push_err: per-request exceptions (config not readable, remote call failure, decoding failure, internal
  unreachable branch)
  Examples: `"token refresh fail"` / `"cache load fail"` / `"jwks fetch:timeout"`
  (push_err's kind has **no** "; "; the runtime auto-completes `"err: <kind>; "`)
kind takes the form `"<action> fail"` or `"<action>:<reason>"`. Both can be called multiple times within the same plugin;
the log is finally serialized as an array:
`log: ["OK user jack", "err: jwks fetch:timeout; ", "OK fallback"]`.

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
