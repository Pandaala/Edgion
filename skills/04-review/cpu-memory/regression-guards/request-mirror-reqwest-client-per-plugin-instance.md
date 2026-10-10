---
name: request-mirror-reqwest-client-per-plugin-instance
description: Use when reviewing findings that flag RequestMirrorPlugin for building its own reqwest::Client in `new()`; this fragments the per-host connection pool, duplicates TLS/DNS caches across plugin instances, and panics on builder failure during plugin load. The fix uses the dedicated process-level `common::http_client::get_mirror_http_client()` singleton without internal timeouts.
knowledge_type: regression-guards
---

> See also: [cpu-memory/SKILL.md](../SKILL.md) for the index.

# `RequestMirrorPlugin` `reqwest::Client` Per-Plugin Instance — FIXED (F-017)

## Conclusion

**Confirmed-real, now fixed (F-017, 2026-05-27).** `RequestMirrorPlugin::new` in
`edgion-gateway/src/plugins/http/request_mirror/plugin.rs` used to build a private
`reqwest::Client` (with `pool_max_idle_per_host(32)`, its own TLS session cache, its own DNS
cache) on every plugin construction and store it in a `client: reqwest::Client` field. Every
route carrying a `RequestMirror` filter therefore owned an independent client; N mirror-enabled
routes sharing the same backend grew the idle-pool footprint to `N * 32` instead of `32`, and
EdgionPlugins hot-reloads dropped and re-created the warm pool each cycle. The constructor also
called `.expect("failed to build RequestMirror reqwest client")`, panicking the gateway at
plugin-load time on builder failure instead of returning a request error.

The fix removes the `client` field entirely and pulls a shared singleton at request time:

```rust
let client = get_mirror_http_client().clone(); // process-level OnceLock from common::http_client
```

`get_mirror_http_client()` is a **dedicated** singleton (not the default `get_http_client()`):
it shares the pool / TLS / DNS state across all `RequestMirrorPlugin` instances, but deliberately
omits both `.timeout(...)` and `.connect_timeout(...)`. The default singleton's hard-coded
`timeout(10s)` + `connect_timeout(5s)` would silently cap any operator who configured
`connect_timeout_ms > 5000` or `connect_timeout_ms + write_timeout_ms > 10000` (the schema has no
upper bound). Per-plugin `connect_timeout_ms` / `write_timeout_ms` bounds are enforced by the
existing outer `tokio::time::timeout(connect_timeout + write_timeout, async { ... })` wrapper in
`run_request`; with no inner reqwest caps, that wrapper is the sole bound and the user-facing
timeout contract is preserved exactly.

Re-reports of the form "RequestMirrorPlugin builds its own reqwest client" or "mirror plugin
fragments the connection pool" are **not accepted**; verify the current `plugin.rs` carries
`get_mirror_http_client().clone()` and has no `client: reqwest::Client` field before re-reporting.
Re-reports of "mirror should reuse `get_http_client()` instead of having its own singleton" are
also **not accepted** — the dedicated singleton exists specifically to avoid the auth client's
total/connect timeout caps clipping operator-configured mirror timeouts.

## Core Rationale

**1. `reqwest::Client` is designed to be shared**

`reqwest::Client` is internally `Arc`-wrapped; cloning hands out a new reference to the same
connection pool, TLS session cache, and DNS cache. The project already standardised on
process-wide `OnceLock<Client>` ownership in
`edgion-gateway/src/plugins/http/common/http_client.rs`; the default `get_http_client()` and the
timeout-free `get_mirror_http_client()` now each share one pool for their distinct contracts.
RequestMirror's former per-instance ownership was the outlier.

**2. The per-plugin differentiator collapses into the outer timeout wrapper**

The only per-plugin knob justifying a dedicated client was `connect_timeout_ms`. The send path
is already wrapped in `tokio::time::timeout(cfg.connect_timeout + cfg.write_timeout, ...)`,
which bounds total send time end-to-end. The dedicated `get_mirror_http_client()` singleton
sets neither `.timeout(...)` nor `.connect_timeout(...)`, so the outer wrapper is the only
timeout source and the per-plugin values are honored exactly. Mirror error classification then
simplifies: there is no reqwest-internal `is_timeout()` source, so all timeouts fall through
the outer `tokio::time::timeout(...)` → `Err(_)` → label `"timeout"` branch consistently.

**3. `.expect()` at plugin-load time is fail-stop, not fail-request**

`RequestMirrorPlugin::new` is called from `plugin_runtime.rs` during route / EdgionPlugins
reconcile. A panic there terminates the gateway process. Switching to the singleton removes
the per-plugin constructor panic path: `get_mirror_http_client()` uses the common singleton
builder's fatal initialization handling once at first use, with a single error log; the failure
surface is centralised and never falls back to the default client's shorter timeouts.

## Intended behavior change (documented, accepted)

- Mirror metric label `connect_timeout` is now unreachable for the dominant timeout path:
  with the mirror singleton having no inner `connect_timeout`, all timeouts surface through
  the outer `tokio::time::timeout(...)` wrapper and are labeled `"timeout"`. The
  `connect_timeout` label is retained in the match arm for the rare case where the underlying
  transport itself surfaces `e.is_timeout()` (e.g. an HTTP/2 protocol-level timeout exposed by
  hyper), but in practice operators should expect the `"timeout"` label for any timeout-class
  failure.

## Re-evaluation Triggers

Re-open only if:

- A future feature requires per-plugin TLS configuration (client cert, custom CA, SNI override),
  per-plugin proxy, or per-plugin `danger_accept_invalid_certs` — in which case the answer is
  a separately reviewed client-reuse policy keyed on the differentiator.
  The canonical `build_webhook_client_with_tls_selection` factory in
  `common::http_client.rs` constructs a dedicated client; it is not a keyed reuse pool.
- The mirror metric label band described above is shown to mislead operations in production.

## Reference Cases

- F-017 (BadLogic, Medium), fixed 2026-05-27.
- Source: `edgion-gateway/src/plugins/http/request_mirror/plugin.rs`.
- Singleton: `edgion-gateway/src/plugins/http/common/http_client.rs::get_mirror_http_client`.
