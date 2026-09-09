---
name: observability-review-rule
description: Use when reviewing observability code changes; provides the checklist covering secrets-in-logs prohibition, label cardinality, metric naming, and log level conventions.
knowledge_type: mandatory-design
---

# Observability Review Rules

> When reviewing code changes, use this checklist to ensure observability design is sound and does not introduce performance issues or information explosion.

## Iron Rule: Logs Must Not Leak Secrets

**Under no circumstances** may tracing/PluginLog/access log/stdout/stderr contain the original of:

- Original request/response body (forbidden even at debug level — once centrally collected, equivalent to permanent archive)
- Secret.data, TLS private key, ACME account private key, HMAC key, API key, Bearer token, JWT, JWE claims original
- User passwords, database passwords, basic auth credentials
- Unfiltered full serialization of `ctx_map` (may contain JWT claims)
- Original plugin config JSON (may contain nested keys)

**Decision method**:

1. Search added/modified files for `tracing::`, `plugin_log.push`, `format!` / `{:?}` / `%` print variables
2. Check whether the printed variables may contain the above sensitive data:
   - Variable name hint: `body`, `content`, `payload`, `raw`, `spec`, `token`, `key`, `secret`, `credential`, `claims`
   - Type hint: `Bytes`, `String` (sourced from HTTP body or K8s resource), `JwtClaims`, `JwkSet`, `PEM`
   - Context hint: Admin API handler, webhook handler, OIDC/JWT plugin, Secret/TLS resource handling
3. If any matches → **must be reworked**:
   - Print only length: `body_len = content.len()`
   - Print only hash or first 8 bytes: `key_prefix = &api_key[..8]`, `sha256(token)[..16]`
   - Delete entirely (if debug value is low)
   - Field filtering: serialize `ctx_map` with allowlist/blocklist

**Common misconfiguration patterns (zero tolerance)**:

| Anti-pattern | Typical code | Correct approach |
|--------|---------|---------|
| Print full request body | `tracing::debug!("Request body: {}", content)` | Print only `body_len` or delete |
| Print full response body | `tracing::debug!("Response: {:?}", resp.body())` | Print only status + content-length |
| Print Secret.data | `tracing::info!(data = ?secret.data, ...)` | Print only `name/namespace` + field key list |
| PluginLog writes original key value | `log.push(&format!("key={}", api_key))` | `log.push(&format!("kl={}", api_key.len()))` |
| `{:?}` entire resource spec | `tracing::debug!(spec = ?route.spec)` | Forbidden even at debug level — print key_name + rv instead |
| `ctx_map` serialized as-is | `serde_json::to_string(&ctx_map)` | Project via an explicit `AccessLogExtern from: ctx` declaration (`EdgionPlugins.spec.accessLogExtern`), resolved only at `logging()` — never inside a request-stage filter |

**Historical cases** (used to identify the same pattern during reviews):

- **#02 Controller create_namespaced Secret request body leak** (found in 2026-04-17 review): `tracing::debug!("Request body: {}", content)` at `controller/api/namespaced_handlers.rs:104`. At debug level, base64 cert private keys and ACME account keys from POST/PUT Secret requests fell into stdout + log archive.
  - Lesson: **Never print the body original in Admin API handlers**, even at debug level; because central collection (Loki/ES) does not distinguish levels — once written, retained.

**Relationship with 03-coding**: This rule is the review-side decision checklist; the corresponding coding standard is in [../../03-coding/01-log-safety.md](../../../03-coding/01-log-safety.md) Iron Rule 1. Keep them in sync when updating.

---

## Iron Rule: Zero Tracing on the Data Plane

The data-plane hot path (`request_filter`, `upstream_peer`, `response_filter`, `proxy_upstream_filter`, TCP/TLS/UDP connection handling, etc.) **forbids any `tracing::*` macros**, including `debug!`.

**Decision method**:

1. Search for `tracing::` calls in added/modified files
2. Determine whether the call site is on the request/connection handling path (vs config load, startup, ConfHandler)
3. If on the hot path → **must be deleted**, switch to one of the alternatives below:

| Scenario | Alternative |
|------|---------|
| Per-request result/decision | `PluginLog` → access log |
| Per-connection or per-request error/event | `ctx.err_log` / `ctx.record_error(EdgionErrorCode::...)` |
| Aggregate counts | `global_metrics().xxx()` |

**Exceptions** (only the following scenarios are permitted):
- Configuration load events in ConfHandler / conf_handler_impl (`tracing::info!`)
- Process-level one-shot initialization on the data plane (`once_cell`)
- Before a fatal error panic (`tracing::error!`)
- Test-infrastructure paths protected by `is_integration_testing_mode()` (e.g., `tracing::warn!` for access_log_store write failure at `pg_logging.rs:89`) — that branch is never entered in production

**Automated enforcement** (`cicd/checks/check_gateway_no_tracing.py`): this rule now has a CI gate. The data/control boundary is *function-level* — one `plugin.rs` holds both `from_config`/`new` (control plane, `tracing` allowed) and the per-request `run_request` (hot path, forbidden) — so the gate works at the function level, not the file level. Over the canonical "Data-plane entry list" files (the dirs/files mirrored in `01-log-safety.md`, including all `plugins/http/*/plugin.rs`, `gateway_api_filters/`, the match engines, `lb/selection`, `backends/`, and the TLS-handshake callbacks), it flags a `tracing` use only when its **enclosing function is not a constructor / config-build / once-init** (recognised by name via the `COLD_FN` knob: `new`, `from_config`, `create`, `build`, `load_*`, `compile_*`, `init_*`, `parse_*`, …). It is **zero-tolerance** (the request/connection functions are already tracing-free), with registered exceptions carrying an inline `// tracing-allow: <reason>` marker. Use `--list` to print current matches. If a genuinely-cold function is ever flagged, rename it to a cold prefix or add its name to `COLD_FN`.

See [coding-standards/01-log-safety.md](../../../03-coding/01-log-safety.md) Iron Rule 3 for details.

---

## Access Log Sanity

### New-Field Review

- [ ] Is the new field **necessary for high-frequency troubleshooting**? Can it be inferred from existing fields?
- [ ] Is the field at **request level**, not plugin-internal intermediate state?
- [ ] Does the field value not cause **storage explosion** (no large bodies, long tokens, unbounded lists)?

### PluginLog Review

- [ ] Each PluginLog entry is ≤ 40 bytes and has no legacy trailing `; `
- [ ] Records only **the final decision** (OK / Deny / Skip / Rate); does not record the process
- [ ] Does not contain full username / token / JWT claims (should be `u=<prefix>***`)
- [ ] Does not stuff information that should be `tracing::debug!` into PluginLog

### ctx_map Review

- [ ] New keys added to `ctx_map` do not contain JWT / OIDC / JWE claims or other sensitive info
- [ ] If sensitive-related keys must be stored, is the access-log projection still safe?
  - The access-log projection of `ctx_map` requires an explicit `AccessLogExtern from: ctx` declaration (`EdgionPlugins.spec.accessLogExtern`) naming the key; nothing reaches the log otherwise. The `ACCESS_LOG_BLOCKED_CTX_KEYS` blocklist (`jwt_claims`/`oidc_claims`/`jwe_payload`) enforcement is two-part: a declaration naming a blocked key is **reported** on the resource's `Accepted=False` status condition at `EdgionPlugins` ingest — admission in this repo is lenient, so that report does NOT keep the resource off the cluster; it still lands in the cache conf_sync serves to the gateway. The actual enforcement is at gateway load: `compile_plugins` (`edgion-gateway/src/plugins/http/plugin_store.rs`, ~line 88) `retain()`s the declared fields and drops any whose `validation_error()` is `Some` before the spec is stored. **When reviewing a change to this blocklist or its enforcement, verify both halves — the ingest report AND the `compile_plugins` `retain()` — the retain is what actually keeps a blocked key out of the log; do not let a change remove it on the theory that ingest already rejects it.** Any new key that stores claims / payloads / secrets for downstream `get_ctx_var` reads SHOULD still be added to that blocklist so it can never be declared even by mistake.

---

## Metrics Sanity

### Avoid Explosion

- [ ] **Histograms must be opt-in**: `histogram!` calls must be guarded by an `enabled` switch and the default value must be `false`; bare `histogram!` calls without a switch are forbidden
- [ ] **Bounded label cardinality** (value range ≤ tens) — never use path, user_id, ip, trace_id as labels
- [ ] Is the new metric truly necessary? Can existing metrics already express it? Do not store extra what can be computed

### Correct Management

- [ ] Managed uniformly via the `GatewayMetrics` struct; do not directly use the `metrics::counter!()` macro to create floating metrics
- [ ] Naming convention: `edgion_<component>_<what>_<unit>_total / _active`
- [ ] Counters use integer `increment(n: u64)`, not floats

### Gauge Symmetry

- [ ] Every Gauge `+1` path has a corresponding `-1` path
- [ ] Prefer RAII guards (`Drop` impl) to guarantee symmetry
- [ ] Review exceptional paths (early return, panic) to check for Gauge drift

### Monitoring Value

- [ ] New metrics answer a specific operations/SRE question (e.g., "error rate of a backend", "active connections")
- [ ] Not added "just in case" — metrics have long-term storage cost

---

## Common False Positives

| Phenomenon | Verdict | Notes |
|------|------|------|
| `tracing::info!` in ConfHandler | ✅ Reasonable | Config load is not on the request hot path |
| Per-event `tracing::info!` co-existing with a batch-level `info!` that already logs all keys | ❌ Noisy — downgrade per-event to `debug!` | Fixed in g9b-008: `cache_client/event_dispatch.rs` + `cache_data.rs` — per-item logs inside watch/compress loops are redundant when a batch summary already aggregates the same keys at `info!` level |
| Plugin only writes `OK` without detailed reason | ✅ Reasonable | A normally passing plugin does not need detailed logs |
| No separate Counter added for each plugin | ✅ Reasonable | Auth/rate-limit results are already reflected in status and access log |
| Access log field has values only in some scenarios | ✅ Reasonable | Use `skip_serializing_if` |
| `tracing::debug!` prints body/secret/token | ❌ Zero tolerance | See "Logs must not leak Secrets" iron rule; forbidden even at debug |
