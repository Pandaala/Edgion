---
name: create-async-logger-typed-log-type
description: create_async_logger now takes a typed LogType parameter (not a &str), so the previous "unreachable default arm silently misroutes unknown log_type to LogType::Access" pattern is closed at the type system level. Reject re-reports of "unreachable _ => LogType::Access arm" or "stringly-typed log_type with no startup guard" — the &str signature no longer exists.
knowledge_type: regression-guards
---

# observe F-B03 — `create_async_logger` takes typed `LogType`, not `&str`

**Status:** ✅ fixed · 2026-05-29
**Severity:** Low (BadLogic / code-quality) — `create_async_logger(config: &LogConfig, log_type: &str)`'s catch-all arm `_ => LogType::Access` was unreachable for the four current callers (`"access"` / `"tcp"` / `"tls"` / `"udp"`), but the `&str` parameter let any future typo or new kind (`"grpc"`, `"http"`, `"Tcp"`) fall through to `LogType::Access` with no compile error, no startup warning, and no tracing event.

## Conclusion

`create_async_logger` (`edgion-gateway/src/observe/logs/logger_factory.rs:53`) now takes `log_type: LogType` directly:

```rust
pub async fn create_async_logger(
    config: &LogConfig,
    log_type: LogType,
) -> Result<Option<Arc<AccessLogger>>>
```

The internal `match log_type { "access" => ..., _ => LogType::Access }` dispatch is gone. `build_writer_config` also takes `LogType` so the disabled-mode `tracing::info!(?log_type, ...)` lines preserve the same kind-identity in logs (Debug format: `Access` / `Tcp` / `Tls` / `Udp` — operator-visible string changes from lowercase to `Debug`-cased, see Re-evaluation Triggers).

The four call sites pass typed enum variants:

| File | Variant |
|---|---|
| `edgion-gateway/src/observe/access_log/mod.rs:25` | `LogType::Access` |
| `edgion-gateway/src/observe/logs/tcp_log.rs:58` | `LogType::Tcp` |
| `edgion-gateway/src/observe/logs/tls_log.rs:18` | `LogType::Tls` |
| `edgion-gateway/src/observe/logs/udp_log.rs:95` | `LogType::Udp` |

`LogType` is now re-exported from `crate::core::gateway::observe::logs` so callers do not reach into `link_sys::providers::local_file` directly.

## Core Rationale

**1. Compiler beats startup guard for a closed kind set.**

The kind set is fixed by `LogType` (`Access` / `Ssl` / `Tcp` / `Tls` / `Udp`) and lives in `link_sys::providers::local_file`. The original finding listed two options — switch to `LogType`, or add `try_from(&str)` that errors at startup. Both close the latent path, but typed enum is strictly cheaper at the call site: no error path for the caller to forget, no per-startup conversion, and the enum has been the actual storage type inside `LocalFileWriter` since day one. The `&str` boundary was an arbitrary serialization step with no source of truth (no config file, no admin API, no on-wire protocol parses kind strings into `create_async_logger`).

**2. The unreachable arm was latent, not theoretical.**

All four current callers pass compile-time literals, so the `_ => LogType::Access` arm could not execute. The hazard was strictly forward-looking: a future protocol logger (e.g., gRPC, HTTP-detail) added to the same module would extend `LogType` first, then add an `init_*_logger` calling `create_async_logger(config, "grpc")`. The new variant would not appear in the match, the catch-all would silently route the new kind's drop counter into `access_log_dropped()` (see `data_sender_impl.rs:49-53`), and the regression would be invisible at startup. Removing the `&str` layer closes the class.

**3. Re-export `LogType` from `observe::logs`, do not import across layers.**

`LogType` is defined in `crate::core::gateway::link_sys::providers::local_file` because the writer owns it for metrics dispatch. The fix re-exports it via `observe::logs::LogType` so the four observe-side callers import it from the same module as `create_async_logger`. Forcing observe-side modules to reach into `link_sys::providers::local_file` for a parameter type of a function in `observe::logs::logger_factory` would be a layering inversion. The re-export is `pub use` only — no new type, no `From` impls.

**4. Symmetry with `create_sync_logger`.**

`create_sync_logger` already hard-coded `LogType::Ssl` internally (it is the only sync sink). The fix keeps that as-is: the sync path never needed a parameter because there is exactly one sync logger kind. `build_writer_config` is shared by both factories and now takes `LogType`; the sync site passes `LogType::Ssl` directly.

## Fix Suggestions Not Accepted

- "Add `LogType::try_from(&str) -> Result<LogType, _>` and propagate the error so mismatches surface at startup." Not accepted — keeping the `&str` boundary preserves the original hazard surface (caller must remember the spelling) for no gain over the typed parameter. The typed enum is the strictly smaller surface and the issue file itself listed it as the preferred option.
- "Keep `&str` but make the default arm `panic!(...)` or return `Err(...)` so unknown kinds blow up at startup." Not accepted — same redundancy as `try_from`, plus shifts a compile-time error into a runtime check on a path the compiler can already enforce.
- "Generate the match table via a procedural macro / `strum::EnumString` derive." Not accepted — adds a build-time dependency to remove a 5-line match that no longer exists after the typed-parameter fix.

## Re-evaluation Triggers

Re-open only if:

- A serialized kind string genuinely enters the call path (config field, admin API, or wire protocol that names a logger kind by string and routes it into `create_async_logger`). At that point a `LogType::try_from(&str)` deserializer is the right boundary, and it lives at the deserialization site — not inside `create_async_logger`.
- The disabled-mode `tracing::info!(?log_type, ...)` Debug format (`Access` / `Tcp` / `Tls` / `Udp`) clashes with an existing observability query keyed on the previous lowercase form (`"access"` / `"tcp"` / `"tls"` / `"udp"`). The fix is then a `Display` impl on `LogType` returning the lowercase form, not a revert to `&str`.

## Reference Cases

- observe F-B03 (`tasks/observe/observe-FB03-create-async-logger-log-type-unreachable-default.md`, closed 2026-05-29)
- [[protocol-logger-oncelock-pattern-not-extracted]] — sibling decision on the four-async-loggers cluster; its rationale about the kind-name appearing in `tracing::info!` literals (`"TcpLogger initialized"` etc.) is unchanged, only the kind-to-LogType resolution step moved from string match to typed parameter
- [[access-log-disabled-init-noop]] — the F-B01 disabled-mode no-op behavior is independent of this fix
- `edgion-gateway/src/link_sys/providers/local_file/mod.rs:14-21` — `LogType` enum definition (source of truth, derives `Copy` so the typed parameter has no clone cost)
- `edgion-gateway/src/link_sys/providers/local_file/data_sender_impl.rs:49-53` — the metrics dispatch that the silently-misrouted `LogType::Access` would have corrupted

## Resolution references

- `observe-FB03` — 2026-05-29.
