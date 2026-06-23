---
name: tcp-log-entry-status-debug-format-replaced
description: Use when reviewing findings that flag `TcpLogEntry::from_context`'s `format!("{:?}", ctx.status)` per-connection String allocation against the 12-variant unit enum `TcpStatus`; the fix is a `TcpStatus::as_str(&self) -> &'static str` inherent method called from the log builder.
---

> See also: [cpu-memory/SKILL.md](../SKILL.md) for the index.

# `TcpLogEntry::from_context` Per-Connection `format!("{:?}", TcpStatus)` — FIXED (observe-F012)

## Conclusion

**Confirmed-real, now fixed (observe-F012, 2026-05-28).** `TcpLogEntry::from_context`
in `edgion-gateway/src/observe/logs/tcp_log.rs` previously serialised
`ctx.status: TcpStatus` into the `status: String` log field with
`format!("{:?}", ctx.status)`. `TcpStatus`
(`edgion-gateway/src/routes/tcp/edgion_tcp.rs`) is a `derive(Debug, Clone)` plain
enum with 12 unit variants (`Success`, `NoRouteMatched`, ..., `DownstreamWriteError`)
and no heap state, so every TCP connection close paid one `format!` + `Debug::fmt`
+ `String` allocation whose content was always one of 12 compile-time-known string
literals — non-HTTP-hot-path but always paid on the TCP-proxy close path.

The fix adds an inherent `as_str(&self) -> &'static str` method on `TcpStatus`
whose `match` returns the same per-variant label that `Debug::fmt` was producing,
and the log builder is now:

```rust
let status = ctx.status.as_str().to_owned();
```

The `String` is still owned (the `TcpLogEntry.status: String` shape is preserved
to keep the entry `'static` for the async log channel), but the allocation
source is now a copy of a known short literal rather than a `Debug` format.

The actual *primary* gain is decoupling the log schema from the `Debug` impl:
operators' downstream log parsers (Elasticsearch / ClickHouse pipelines) are
keyed on the literal strings `"Success"`, `"NoRouteMatched"`, .... With the old
`format!("{:?}", ..)` form, renaming a `TcpStatus` variant for code clarity
would silently change the log field — caught by no test. With `as_str()`, the
label table is explicit, and the regression test
`tcp_status_as_str_matches_log_field` (`tcp_log.rs` `tests` module) locks every
variant's string. Future variant additions are caught at compile time by the
exhaustive `match` in `as_str()`.

Re-reports of the form "`TcpLogEntry::from_context` uses `format!("{:?}", ..)`
on a unit enum" / "TCP access log status should be `&'static str`" are **not
accepted**; verify the current `tcp_log.rs:from_context` carries
`ctx.status.as_str().to_owned()` and `edgion_tcp.rs` exposes
`TcpStatus::as_str` before re-reporting.

## Core Rationale

**1. The label set is closed and compile-time-known.**

`TcpStatus` is a plain enum with 12 unit variants and no heap state; the label
"set" is fixed by the type definition. `format!("{:?}", ..)` runs the full
`Debug::fmt` + `fmt::Write` machinery to produce a string already known at
compile time.

**2. The `Debug` impl is not a stable log contract.**

Using `format!("{:?}", ..)` to populate a persistent log field couples the
access-log schema to the auto-derived `Debug` output. A future rename of a
variant for code clarity would silently change the access-log field — operators
consuming the log via field-equality filters (`status == "PluginDenied"`) would
break with no compiler warning and no test failure. `as_str()` makes the
contract explicit and the regression test pins each label.

**3. The `String`-typed field is intentional, not the bug.**

`TcpLogEntry.status: String` is owned because the entry crosses an async
channel into the log writer task and must be `'static`. The fix does not change
the entry shape (no `Cow<'static, str>` plumbing through the writer / serde
adapter) — it only changes the *source* of the string from a per-call `format!`
allocation to a copy of a static literal. Net allocation count per TCP close is
unchanged; the saving is on the `format!` machinery, and the structural win is
the explicit label contract.

**4. Path classification.**

TCP connection close — non-HTTP hot path but always paid on TCP-proxy
deployments. The `format!` micro-cost is small per-call; the regression
surface ("variant rename silently changes log field") was the bigger latent
risk, which the explicit `as_str()` + locking test now removes.

## Re-evaluation Triggers

Re-open only if:

- A consumer of the TCP access log starts comparing the `status` field against
  the *new* `Debug` output of a future variant rename (i.e. someone deliberately
  diverges the `as_str()` label from the variant identifier and forgets to
  update the test) — in which case the fix is to update both in lock-step, not
  to re-introduce `format!("{:?}", ..)`.
- The whole `TcpLogEntry` is refactored to a `Cow<'static, str>` /
  `&'static str` schema across the async-channel boundary (changes the
  allocation pattern entirely; revisit at that point).

## Reference Cases

- observe-F012 (BadLogic / Minor), fixed 2026-05-28.
- Source: `edgion-gateway/src/observe/logs/tcp_log.rs`,
  `edgion-gateway/src/routes/tcp/edgion_tcp.rs`.
- Regression test: `tcp_status_as_str_matches_log_field` in
  `tcp_log.rs::tests` module.
