---
name: protocol-logger-oncelock-pattern-not-extracted
description: Decision record for observe F-010 — the OnceLock + init_*_logger + get_*_logger + log_* three-piece pattern repeated across tcp_log.rs / tls_log.rs / udp_log.rs (with ssl_log.rs as a near-clone over a different backend) is intentionally not extracted into a `ProtocolLogger<T>` wrapper or macro.
---

# observe F-010 — Protocol logger OnceLock pattern is not extracted

## Conclusion

Four async sites already exist over `Arc<AccessLogger>`:

- `src/core/gateway/observe/access_log/mod.rs:15-42`
  (`ACCESS_LOGGER` / `init_access_logger` / `get_access_logger` plus the extra
  `get_access_logger_unchecked()` panic-on-miss helper used by callers that
  initialize early and want to skip the `Option` dance)
- `src/core/gateway/observe/logs/tcp_log.rs:11,57-77`
- `src/core/gateway/observe/logs/tls_log.rs:14,17-40`
- `src/core/gateway/observe/logs/udp_log.rs:12,94-116`

Each declares:

1. `static *_LOGGER: OnceLock<Arc<AccessLogger>>`
2. `pub async fn init_*_logger(config: &LogConfig) -> Result<()>` — calls
   `create_async_logger(config, "<kind>")`, `OnceLock::set(...)`, and emits two
   `tracing::info!` lines with `"<Kind>Logger initialized"` /
   `"<Kind>Logger disabled"`.
3. `pub fn get_*_logger() -> Option<&'static Arc<AccessLogger>>` and
   `pub async fn log_*(entry)` that early-returns when the `OnceLock` is empty.

`src/core/gateway/observe/logs/ssl_log.rs:16,55-73` follows the same outer
shape over a different backend (`SslLogger` with a `mpsc::Sender<String>` +
sync `try_send`, bumps `global_metrics().ssl_log_dropped()` on overflow — not
`Arc<AccessLogger>`).

Folding the four async variants into a `ProtocolLogger<T>` struct / macro is
**not accepted**. Findings of the form "extract a `ProtocolLogger<T>` /
`ProtocolLoggerCell<T>` / `init_get_log!` macro that wraps the OnceLock
lifecycle" should be closed as ⚠️ accepted tradeoff without code change.

## Core Rationale

**1. Pure redundancy, no behavioral defect.**

The four async files have identical text-level shape but have not drifted:
`OnceLock::set(...).map_err(|_| anyhow!("<Kind>Logger already initialized"))`
(or `"AccessLogger already initialized"` / `"Global AccessLogger initialized"`
for the access_log site — the kind-name strings are the only intentional
per-site variation), the if/else `tracing::info!("<Kind>Logger initialized"
/ "<Kind>Logger disabled")`, and the `OnceLock::get()` early-return are
consistent across `access_log/mod.rs`, `tcp_log.rs`, `tls_log.rs`,
`udp_log.rs`, so the "future-drift hazard" the refactor is supposed to
prevent has not appeared in practice. The shared real work (writer
construction, `LocalFileWriter::init`, queue/rotation wiring) already lives
in `logger_factory.rs::create_async_logger`, which all four sites call with
their own `LogType` enum variant (`LogType::Access` / `Tcp` / `Tls` / `Udp`
— typed since observe-FB03, 2026-05-29; the kind is no longer routed
through a string match). What's left in each file is the per-protocol
OnceLock plumbing.

**2. The wrapper cannot hold the static — per-protocol boilerplate stays.**

Stable Rust does not support a generic `static FOO: OnceLock<T>` keyed on the
wrapper type without a per-concrete-type declaration. Any `ProtocolLogger<T>`
abstraction must still take `&'static OnceLock<Arc<AccessLogger>>` (or
equivalent) as a constructor argument, so each of `access_log/mod.rs`,
`tcp_log.rs`, `tls_log.rs`, `udp_log.rs` would keep its own
`static *_LOGGER: OnceLock<...> = OnceLock::new();` declaration plus a
`pub async fn init_*_logger` / `log_*` (and, for access_log, `get_*` +
`get_*_unchecked`) thin wrapper to keep the existing public API stable
(`mod.rs` re-exports `init_*_logger`, `log_*`, plus the per-protocol entry
types `TcpLogEntry` / `UdpLogEntry`; the entry types themselves cannot be
subsumed by a wrapper). The net saving is on the order of 10 lines per file
— about 40 lines total across the four async sites — at the cost of one new
abstraction reads have to chase.

**3. `ssl_log.rs` cannot share the wrapper.**

`SslLogger` is a `struct` with its own `tx: mpsc::Sender<String>` and a
`spawn`-ed receiver loop (`src/core/gateway/observe/logs/ssl_log.rs:21-52`);
its `log(&self, entry: &TlsConnMeta)` is **sync** (must be safe to call from
TLS callbacks), and on `try_send` failure it bumps
`global_metrics().ssl_log_dropped()`. None of that fits an
`Arc<AccessLogger>`-shaped wrapper. Even after a successful extraction the
codebase would have "four async loggers via `ProtocolLogger<T>` + one
`SslLogger`" rather than "five similar files" — that is not a clearer story
than the current state.

**4. Log-string and entry-type divergence already keeps shapes from
collapsing cleanly.**

- The `tracing::info!` literals encode the kind name (`"TcpLogger initialized"`,
  `"TlsLogger disabled"`, `"UdpLogger initialized"`, `"Global AccessLogger
  initialized"`, `"AccessLogger disabled"`). A macro either takes the kind as
  a literal parameter (each call site still writes the literal) or uses
  `std::any::type_name::<T>()` and changes production log strings to
  compiler-internal names. Past precedent
  (`infra-cluster-F012-conf-handler-simple-map-pattern.md`) declined the same
  trade-off.
- `log_tcp(entry: &TcpLogEntry)` / `log_udp(entry: &UdpLogEntry)` go through
  the concrete `entry.to_json()` helpers, whereas `log_tls<T: Serialize>(&T)`
  is already generic and calls `serde_json::to_string(entry)` directly, and
  `access_log` writes pre-built `AccessLogEntry` JSON through `AccessLogger`'s
  own send API. The payload-shaping step is therefore not uniform across the
  four async loggers.
- `access_log/mod.rs` additionally exposes `get_access_logger_unchecked()`
  (panic on miss), used by callers that initialize early and want to skip the
  `Option` dance. A generic `ProtocolLogger<T>` would either omit this
  helper (forcing access_log callers to bring their own) or add it for every
  protocol (extending the API surface of `tcp_log` / `tls_log` / `udp_log` for
  no current consumer).

**5. Established precedent class.**

This finding sits in the same class as:

- `architecture/style-consolidation-thresholds.md` — "Three similar lines is
  better than a premature abstraction; `Looks duplicated` / `style
  inconsistent` must be supported by real maintenance cost or reading-cost
  evidence."
- `architecture/infra-cluster-F012-conf-handler-simple-map-pattern.md` — same
  shape of finding (extract a generic wrapper around a
  `static + init + accessor` pattern across 2-3 sites), closed ⚠️ accepted
  tradeoff for the same wrapper-cannot-hold-the-static and kind-literal
  reasons.
- `cpu-memory/cases/oncelock-global-singleton-deliberate-design.md` — the
  `OnceLock<Arc<...>>` lifetime matching process lifetime is itself a
  deliberate design choice in this codebase.

## Fix Suggestions Not Accepted

- "Introduce `ProtocolLogger<T>` that wraps `OnceLock<Arc<AccessLogger>>` and
  expose generic `init` / `get` / `log` methods" — per-file `static OnceLock`
  declarations and `pub async fn init_*` / `log_*` shim wrappers stay; the
  remaining savings (~40 lines across the four async sites) do not justify
  the new abstraction layer.
- "Replace the three protocol loggers with a single declarative macro
  (`define_protocol_logger!(tcp, TcpLogEntry, ...)`)" — same saving,
  additionally hides the `OnceLock::set` failure path and the `tracing::info!`
  literals from grep, and re-introduces kind-name coupling at the macro level.
- "Fold `ssl_log.rs` into the same abstraction by parameterizing over the
  backend" — the sync `try_send` + `ssl_log_dropped` metric semantics differ
  from the async `AccessLogger::send`; an abstraction broad enough to cover
  both leaks backend details into the wrapper's API.

## Re-evaluation Triggers

Re-open this decision only if:

- A **fifth** async protocol logger is added on top of the existing four
  (`access_log` + `tcp` / `tls` / `udp`), keeping the same
  `OnceLock<Arc<AccessLogger>>` + `init` + `get` + `log` shape — at five
  identical async sites, "drift prevention" starts to bite enough to revisit.
- The four existing sites' init/disable log strings, error messages, or
  double-initialization semantics genuinely **drift** and the drift breaks a
  production observability query (per
  `style-consolidation-thresholds.md` — drift-prevention is the only
  acceptance gate for pure-DRY refactors).
- `init_*_logger` grows a new mandatory step (e.g., metrics registration,
  cross-protocol coordination) that all four sites must adopt — at that point
  the shared step is the real abstraction, not the OnceLock cell.

## Reference Cases

- Closed finding: `tasks/observe/observe-F010-protocol-loggers-four-identical-onceLock-init-pattern.md` (closed 2026-05-28, ⚠️ accepted tradeoff).
- Sources kept as-is:
  - `src/core/gateway/observe/access_log/mod.rs:15-42`
  - `src/core/gateway/observe/logs/tcp_log.rs:11,57-77`
  - `src/core/gateway/observe/logs/tls_log.rs:14,17-40`
  - `src/core/gateway/observe/logs/udp_log.rs:12,94-116`
  - `src/core/gateway/observe/logs/ssl_log.rs:16,55-73`
- Shared real work (intentional consolidation point):
  `src/core/gateway/observe/logs/logger_factory.rs` — `create_async_logger`
  (called by access_log / tcp_log / tls_log / udp_log with the typed
  `LogType::Access` / `Tcp` / `Tls` / `Udp` enum variants, observe-FB03,
  2026-05-29), `create_sync_logger` (called by ssl_log, hard-wired to
  `LogType::Ssl`).
