---
name: response-body-size-idempotent-finalization
description: Use when reviewing duplicate response_body_size writes from HTTP proxy failure callbacks and final logging.
status: fixed
finding_id: gw-http-proxy-resp-02
closed: 2026-08-02
knowledge_type: regression-guards
---

# Response body size finalization is idempotent — FIXED

## Conclusion

`UpstreamInfo::set_response_body_size` treats a repeated identical byte count as
an idempotent finalization. It reports `response_body_size already set` only
when a later writer supplies a different value.

Fix suggestions that remove failure-handler accounting or suppress every
duplicate write are **not accepted**.

## Core Rationale

**1. Failure callbacks and final logging legitimately observe the same attempt.**

`fail_to_connect` and `error_while_proxy` record the attempt's body size when
the attempt terminates. If no retry follows, `pg_logging::logging` finalizes the
same `UpstreamInfo` with the same session byte count. That lifecycle produces
two writes without indicating inconsistent accounting.

**2. Retried attempts still need failure-time accounting.**

`begin_upstream_attempt` advances the context to a new `UpstreamInfo`, so final
request logging cannot populate an earlier failed attempt. Removing the
failure-handler writes would make retry history incomplete.

**3. A changed value remains diagnostic.**

The setter preserves its previous last-write-wins behavior and appends the
diagnostic when two writers disagree. This removes routine access-log noise
without hiding a real accounting conflict.

## Fix Suggestions Not Accepted

- Remove body-size recording from failure callbacks — retried attempts would
  lose their final byte count after the context advances.
- Ignore all repeated writes — a conflicting later count would no longer leave
  an access-log signal.
- Add call-site-specific duplicate checks — that would distribute the setter's
  idempotence rule across proxy lifecycle callbacks.

## Re-evaluation Triggers

Re-open only if:

- Pingora changes callback ordering so bytes can continue arriving after an
  attempt's error callback.
- Response body accounting changes from a final per-attempt value to a
  deliberately progressive counter.

## Reference Cases

- Source finding: `tasks/fable-review/gw-http-proxy-resp-02-response-body-size-double-set-false-positive.md`.
- Setter: `edgion-gateway/src/ctx.rs::UpstreamInfo::set_response_body_size`.
- Callers: `pg_fail_to_connect.rs`, `pg_error_while_proxy.rs`, and
  `pg_logging.rs`.
- Access-log contract: `skills/03-coding/observability/00-access-log.md`.

## Resolution references

- `gw-http-proxy-resp-02` — 2026-08-02.
