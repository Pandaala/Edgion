# Task: Center admin audit log (persisted to local SQLite)

> **Repositioned:** rich audit (storage/UI/retention/multi-tenant) belongs to the
> **Management Plane** (separate optional component) — see
> `center-split-hub-management-plane.md`. The lean **Federation Hub** only emits structured
> `tracing` audit events for its own actions (no DB dependency). The SQLite-backed parts below
> apply to the Management Plane, not the Hub.

**Profile:** feature / single-file
**Status:** todo (not started)
**Depends on / relates to:** `center-build-in-frontend.md` (the embedded dashboard is the
main interactive client whose actions need auditing).

## Why

Center authz is intentionally **"login = admin"** for now — no per-user RBAC; anyone who can
authenticate (Okta/OIDC or `local_auth`) has full Center admin access, capped only by the
controller→center fed ceiling (`center_role`). Given there is **no per-user authorization**,
an **audit log is the compensating control**: it records *who did what, when, from where, and
with what result*, so privileged actions are attributable even though everyone is an admin.

## Goal

Record Center admin actions with attribution. To avoid a **strong single-DB dependency**
(multiple independent Centers, no shared store — see `center-auth-rbac-design.md`):

- **Primary sink = structured `tracing` events** (`component = "audit"`), carried by the
  platform's existing log pipeline (stdout → log system / SIEM). Durable, per-Center,
  **needs no sync** — aggregate centrally at the log layer.
- **Local SQLite (`CenterDb`) = optional browse-cache** for the dashboard's in-product audit
  view; best-effort, losing it does not lose the compliance trail (the log pipeline has it).

## What to audit (v1)

- **Mutating admin operations**: `POST` / `PUT` / `DELETE` / `PATCH` on Center business
  routes — failover, GlobalConnectionIpRestriction create/update/delete/enable/active-profile/
  sync, controller reload, admin controller delete, cache clear, sync trigger, etc.
- **Proxied controller operations**: requests under `/api/v1/proxy/{controller_id}/...` —
  record the **target `controller_id`** (decode `~`→`/`) plus the proxied method/path.
- **Auth events**: login success, logout. (Login *failures* — see caveat below.)
- **Reads** (`GET`): **not** audited by default; gate behind an optional `audit.log_reads`
  flag (off by default) to avoid noise.

## Record fields

| column | source |
|---|---|
| `id` | autoincrement PK |
| `ts` | `unix_now()` (seconds) |
| `actor` | `UnifiedAuthClaims.sub` (OIDC) or local username; `<unknown>` if absent |
| `provider` | `local` / `oidc` (from `UnifiedAuthClaims.provider`) |
| `method` | HTTP method |
| `path` | request path (Center route or proxied path) |
| `target_controller` | parsed from `/api/v1/proxy/{id}/...`, `~`→`/`; NULL for Center-local ops |
| `status` | response status code |
| `source_ip` | TCP peer (`ConnectInfo<SocketAddr>`); **X-Forwarded-For never trusted** (mirror `ip_allowlist`) |
| `request_id` | optional correlation id if present |
| `detail` | optional short note / error message (nullable) |

## Schema (add to `SCHEMA_SQL` in `src/core/center/db/mod.rs`)

```sql
CREATE TABLE IF NOT EXISTS audit_log (
    id                INTEGER PRIMARY KEY AUTOINCREMENT,
    ts                INTEGER NOT NULL,
    actor             TEXT    NOT NULL DEFAULT '<unknown>',
    provider          TEXT    NOT NULL DEFAULT '',
    method            TEXT    NOT NULL,
    path              TEXT    NOT NULL,
    target_controller TEXT,
    status            INTEGER NOT NULL,
    request_id        TEXT,
    detail            TEXT
);
CREATE INDEX IF NOT EXISTS idx_audit_log_ts ON audit_log(ts);
CREATE INDEX IF NOT EXISTS idx_audit_log_actor ON audit_log(actor);
```

`CREATE TABLE IF NOT EXISTS` keeps `run_migrations()` idempotent (existing pattern).

## DB methods (`CenterDb`)

- `insert_audit(record: AuditRecord) -> Result<(), rusqlite::Error>` — single insert.
- `list_audit(filter, limit, offset) -> Result<Vec<AuditRecord>, rusqlite::Error>` — paginated,
  optional filters (actor, target_controller, time range), `ORDER BY ts DESC`.
- (optional) `prune_audit(before_ts | keep_max_rows)` — retention.

Follow the existing `conn.lock()` + `rusqlite::params![...]` style; call from async via
`tokio::task::spawn_blocking` like `list_controllers`.

## Where to hook (capture identity + final status)

Add an **audit axum middleware** that reads `UnifiedAuthClaims` from request extensions
(injected by `unified_auth`), runs the inner handler, captures the response status, then
records the event.

- **Placement (critical):** the layer must sit **inside** `unified_auth` so the claims are
  present. Apply it to the **`business` router BEFORE `compose_admin_routes`** (compose wraps
  business with `unified_auth` outermost → claims are injected before the inner business
  layer runs). Do **not** add it after compose (it would run outside auth / miss claims).
  See `src/core/center/cli/mod.rs` (`router(api_state)` → `compose_admin_routes`).
- **Do not block the response on the DB write.** Use a bounded `mpsc` channel + a background
  writer task that drains it and inserts (optionally batched). The middleware just builds the
  record and `try_send`s it; on a full channel, drop + increment a dropped-counter metric
  (never apply backpressure to admin requests).
- **source_ip**: extract from `ConnectInfo<SocketAddr>` (the make-service already provides it
  for the IP allowlist). Never read `X-Forwarded-For`.

### Caveat: failed-login auditing
`unified_auth` rejects bad credentials (401) **outside** the inner business layer, so an
inner audit middleware won't see failed logins. For v1, emit login success/logout from the
auth handlers (or a thin wrapper). If failed-login auditing is required, add a small outer
layer or hook `local_auth`/OIDC validation — note this as a follow-up.

## Config (reuse existing `database`)

- Audit uses the existing `CenterDb`; **requires `database.enabled = true`**. If the DB is
  disabled, log a startup WARN and run without audit (or make audit failure fail-closed — see
  open question).
- Add an optional `audit` config block:
  - `enabled: bool` (default `true` when DB enabled)
  - `log_reads: bool` (default `false`)
  - `retention_days` or `max_rows` (optional pruning; default unbounded + WARN, or a sane cap)

## Read API (for the dashboard)

- `GET /api/v1/center/admin/audit-logs` — paginated (`limit`/`continue_token` or
  `limit`/`offset`), filters: `actor`, `controller`, `since`/`until`. Returns `ListResponse`.
- This endpoint is itself a read — exclude it from `log_reads` to avoid self-logging loops.

## Open questions (resolve during design)

1. **Fail-open vs fail-closed on audit-write failure**: if the audit insert fails (disk full,
   DB locked), should the admin action still succeed (fail-open, audit best-effort) or be
   rejected (fail-closed, "no audit → no action")? Compliance-heavy setups want fail-closed;
   default suggestion: **fail-open + dropped-record metric + WARN**, since audit is a
   compensating control, not the primary gate. Confirm.
2. **Retention**: unbounded growth vs pruning policy + default.
3. **Read scope**: audit only mutations, or also a configurable read trail?

## References
- DB: `src/core/center/db/mod.rs` (`CenterDb`, `SCHEMA_SQL`, `run_migrations`, `unix_now`)
- Auth claims: `src/core/common/unified_auth/mod.rs` (`UnifiedAuthClaims` — `sub`, `provider`)
- Router wiring: `src/core/center/cli/mod.rs`, `src/core/common/api/compose.rs`
- IP/peer handling (source_ip, no XFF): `src/core/common/api/ip_allowlist.rs`
- Proxy path semantics (`~`→`/`, controller_id): `src/core/center/api/mod.rs` (`proxy_handler`)
