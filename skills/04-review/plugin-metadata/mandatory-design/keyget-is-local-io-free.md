---
name: keyget-is-local-io-free
description: Use when reviewing async KeyGet, webhook-in-condition, per-plugin webhook field, or a second extraction contract; KeyGet is a local synchronous I/O-free read and outbound fetch is WebhookKeyGet.
status: accepted-tradeoff
finding_id: condition-layer-s6
closed: 2026-08-23
knowledge_type: mandatory-design
---

# KeyGet Is Local, Synchronous, and I/O-Free — DESIGN-TRADEOFF

## Conclusion

`KeyGet` is a local, synchronous, I/O-free read. Anything that performs I/O is
a plugin (`WebhookKeyGet`), which writes `KeyGet::Ctx` for later readers.
Fix suggestions that re-add an async accessor, a second extraction contract
(`key_get_local` / `evaluate_sync*`), an outbound call behind a condition, an
async per-chunk body filter, or a per-plugin webhook field are **not accepted**.

## Core Rationale

**1. One extraction contract, nine local variants.**

After S12, `KeyGet` has nine variants (`RemoteIp`, `Header`, `Cookie`,
`Query`, `Path`, `Method`, `Ctx`, `RemoteIpAndPath`, `RequestPath`). `PluginSession::key_get`
is one synchronous method. There is no `KeyGet::Webhook` and no dual
`key_get` / `key_get_local` pair. Outbound resolution is `WebhookKeyGet`
writing a context variable; downstream plugins read `{ type: ctx, name: … }`.

**2. A condition must not grow an outbound hop.**

`PluginConditions` evaluate on every request (and, on the body path, every
chunk). Putting an HTTP call behind `skip`/`run` would make a local boolean
gate perform I/O, reintroduce the dual contract this task deleted, and
force the per-chunk body filter to become async for one consumer. The
deleted `ai_proxy` refusal stands: `tokenQuota.key[{index}] must not use a
webhook because quota admission cannot add an outbound dependency`.

**3. Operator-owned plugin order, not a cross-plugin checker.**

A misordered `requestPlugins` list can produce a silent auth bypass
(`WebhookKeyGet` after `KeyAuth`: the ctx var is empty, so KeyAuth 401s or
authenticates a client-supplied header/query and never waits for the
webhook). W-1 deliberately declined a cross-plugin ordering check; the
operator owns order. See
`docs/en/user-guide/http-route/filters/edgion-plugins/webhook-key-get.md`.

## Fix Suggestions Not Accepted

- Re-add an async accessor "just for the async stages" — restores the dual
  contract (`key_get` vs `key_get_local`) that S5 deleted.
- Reintroduce a second extraction contract (`key_get_local` /
  `evaluate_sync` / `evaluate_detail_sync`) — one sync `evaluate` (log-less,
  body/dye) and one `evaluate_detail` (logged, request/response) is the
  whole surface.
- Put an outbound call behind a condition, in any form — a condition is a
  local gate; I/O belongs in `WebhookKeyGet`.
- Make the per-chunk response body filter async to accommodate one outbound
  consumer — the body phase is allocation-free and log-less
  (`PluginConditions::evaluate`, never `evaluate_detail`).
- Give an individual plugin its own webhook field to avoid the ordering
  requirement — it reopens the scattering this task closes and invalidates
  `ai_proxy`'s existing refusal (`quota admission cannot add an outbound
  dependency`).

## Re-evaluation Triggers

Re-open this decision only if:

- A new transport or plugin stage cannot read a ctx var written by an
  earlier `WebhookKeyGet` and has a documented reason it cannot be reordered.
- The operator-owned ordering contract is replaced by an enforced
  cross-plugin dependency graph (the W-1 decline is reversed).
- `KeyGet` itself is redesigned so a variant is no longer a local read.

## Reference Cases

- `condition-layer-s6` (2026-08-23) — webhook extraction cleanup; this entry.
- Operator ordering: `docs/en/user-guide/http-route/filters/edgion-plugins/webhook-key-get.md`.
- Unification: `skills/01-architecture/00-common/05-unification-points.md` §8.
- Sibling hide-credentials retarget: `skills/04-review/key-auth/regression-guards/hide-credentials-query-cookie-strip.md`.

## Resolution references

- `condition-layer-s6` — 2026-08-23 (accepted tradeoff).
