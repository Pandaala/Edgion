---
name: cli-http-client-idle-pool-disabled
description: Use when reviewing findings that flag edgion-cli's outbound HTTP client for its idle-connection pool setting; records why `pool_max_idle_per_host(0)` (no keep-alive) was a real defect for multi-request subcommands and why the fix is a small explicit bound rather than reqwest's unbounded default.
status: fixed
finding_id: EDG-CLI-002
closed: 2026-07-26
knowledge_type: regression-guards
---

> See also: [cpu-memory/SKILL.md](../SKILL.md) for the index.

# `edgion-cli` Outbound Client Idle Pool Disabled — FIXED (EDG-CLI-002)

## Conclusion

**Confirmed-real, now fixed (EDG-CLI-002, 2026-07-26).** `build_cli_client()` in
`edgion-cli/src/cli/client.rs` set `pool_max_idle_per_host: Some(0)`, so every HTTP
round-trip closed its socket and the next request in the same process paid a fresh TCP
(and, over HTTPS, TLS) handshake. The value carried no security or correctness rationale —
it has been present since the CLI client was first written, annotated only with
`// Disable connection pooling`. It is now `Some(2)`.

Re-reports of the "CLI disables keep-alive" form are moot. Re-reports of the opposite form
("the CLI should not pool connections at all") are **not accepted** without a concrete
threat or resource argument (see Re-evaluation Triggers).

## Core Rationale

**1. The CLI genuinely runs multi-request commands**

A CLI process builds one `Client` (both `EdgionClient::new` and
`EdgionClient::with_static_bearer` route through `build_cli_client`) and reuses it for the
whole command. Two shipping paths issue many requests against it:

- `edgion-cli get --all` auto-paginates in a loop until `continue_token` is null
  (`commands/get.rs`), one request per page.
- `edgion-cli apply -f <multi-doc.yaml>` calls `apply_doc` per YAML document, and each
  document issues a probe `GET` followed by a `PUT`/`POST` (`commands/apply.rs`) —
  2N requests for N documents.

With a zero-sized idle pool each of those requests re-dialed. Against a remote controller
over HTTPS that is one extra RTT for the TCP handshake plus the TLS handshake RTTs, paid
per request, for no benefit.

**2. A small explicit bound, not reqwest's default**

The fix is `pool_max_idle_per_host: Some(2)` rather than dropping the knob and inheriting
reqwest's default (effectively unbounded idle entries + 90s idle timeout). A CLI process
talks to exactly one host and issues requests serially, so 2 is enough to keep the
connection warm across any command, while keeping the transport policy explicit and
reviewable next to the factory's other deliberate deviation from the server-side call sites
(`disable_proxy: false` — every other `build_outbound_client` caller passes `true`), which
states its reason inline. `redirect: Deny` is not a deviation; it is the shared factory's
default and matches every call site.

One nuance the bound does not remove: `apply_doc` drops the probe `GET` response without
reading its body, so hyper only returns that connection to the pool when the unread body
drains in a single best-effort read. Reuse across the probe is therefore best-effort;
`get --all` fully drains each page body (`parse_json_response`) and reuses unconditionally.

**3. Pooling here carries no cross-tenant risk**

reqwest pools per host. The CLI has a single `base_url` for the process lifetime and its
bearer token is fixed at construction (`with_static_bearer`), so a reused connection can
never serve a different host or a different credential. The process is short-lived, so
idle entries are released at exit rather than lingering.

## Fix Suggestions Not Accepted

- "Remove `pool_max_idle_per_host` and take reqwest's default" — loses the explicit,
  reviewable bound and the inline rationale that the rest of this factory carries; an
  unbounded idle pool buys nothing for a single-host serial client.
- "Make the pool size a CLI flag / config field" — no operator has a reason to tune a
  short-lived single-host client; a knob would be dead configuration surface.
- "Build the client through a process-level `OnceLock` singleton like the gateway does" —
  the gateway singleton exists because *many plugin instances* would otherwise fragment
  one pool (see [request-mirror-reqwest-client-per-plugin-instance.md](request-mirror-reqwest-client-per-plugin-instance.md));
  a CLI process constructs at most one `EdgionClient`, so there is nothing to share.

## Re-evaluation Triggers

Re-open this decision only if:

- The CLI gains a concurrent/parallel request mode (e.g. parallel apply across documents),
  where 2 idle entries could become a bottleneck.
- The CLI gains the ability to talk to more than one host or to rotate the bearer token
  within a single process, which would change the "one host, one credential" premise.

## Reference Cases

- `tasks/review-issues/cli-02-unpooled-http-client-keepalive-disabled.md` (EDG-CLI-002,
  closed 2026-07-26)
- `edgion-cli/src/cli/client.rs` — `build_cli_client()`
- `edgion-common/src/link_sys_core/http_client.rs` — the canonical
  `build_outbound_client` factory (design rule D.1)
- [request-mirror-reqwest-client-per-plugin-instance.md](request-mirror-reqwest-client-per-plugin-instance.md)
  — the gateway-side client-pooling finding of the same family

## Resolution references

- `EDG-CLI-002` — 2026-07-26.
