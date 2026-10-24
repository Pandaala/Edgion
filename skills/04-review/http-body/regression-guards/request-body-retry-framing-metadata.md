---
name: request-body-retry-framing-metadata
description: Use when reviewing request-body mutation and retry framing; covers why immutable final-body metadata must outlive the plugin-readable body view.
status: fixed
finding_id: BODY-001
closed: 2026-07-27
knowledge_type: regression-guards
---

# BODY-001 — Request-Body Retry Framing Metadata — FIXED

## Conclusion

When a request plugin commits a body mutation, the final body length must remain
in the request context until all upstream attempts finish. Every attempt removes
`Transfer-Encoding` and writes `Content-Length` from that immutable value, even
after the plugin-readable `BodyStorageView` has been released.

Fix suggestions that retain the plugin-readable view for the full retry
lifecycle or give plugins and upstream replay a shared cursor are **not
accepted**.

## Core Rationale

**1. Pingora rebuilds headers for every attempt**

Pingora clones the original downstream request header and invokes
`upstream_request_filter` again for each HTTP/1 or HTTP/2 upstream attempt. A
framing correction applied only to the first clone is therefore not durable.

**2. Body bytes and framing have different lifetimes**

`BodyStorage` owns the final immutable version and rewinds its replay cursor for
each retry. The plugin-readable `BodyStorageView` can be released after the
request plugin stage, but the final length is still needed to rebuild every
attempt's headers. The current owner is `EdgionHttpContext::request_body` (`RequestBodyState`),
whose private backend retains `modified_len: Option<u64>` separately from its
plugin-readable view. `modified_len_for_framing` reads the live modified length
when a view exists; `release_view` preserves that scalar before dropping the view.
Later attempts recover the retained length without extending plugin access.

**3. Unmodified requests preserve streaming semantics**

The length is recorded only when the body version is modified. Requests whose
body is unchanged keep their original `Content-Length` or
`Transfer-Encoding`, so the framing fix does not turn ordinary upstream request
bodies into a new buffering mode.

## Fix Suggestions Not Accepted

- Keep the plugin-readable view until retries finish — unnecessarily extends
  plugin access; retaining the RequestBodyState framing scalar is required.
- Share one mutable cursor between plugins and upstream replay — couples
  serial plugin reads to transport retries and mutation ordering.
- Patch only the first upstream request header — Pingora creates a fresh clone
  for every retry.

## Re-evaluation Triggers

Re-open this decision only if:

- Pingora stops rebuilding the upstream request header for each attempt.
- A later request-stage mutation is allowed after upstream replay has started.
- Request-body framing gains additional attempt-stable metadata beyond the
  final byte length.

## Reference Cases

- Origin finding:
  `tasks/review-body-buffering-runtime/body-001-retry-framing.md`
- Implementation:
  `edgion-gateway/src/routes/http/proxy_http/pg_upstream_request_filter.rs`
- Request context: `edgion-gateway/src/ctx.rs`; framing owner:
  `edgion-gateway/src/request_body/state.rs` and `state/enabled.rs`.
- Unit regressions: `pg_upstream_request_filter_test/body_framing_tests.rs`
  rebuilds a retry header after view release; `request_body/state_test.rs`
  verifies released plugin access and retained mutation length.
- Integration coverage: `EdgionPlugins_BodyPlugins` shorter HTTP/1 and longer
  HTTP/2 mutation retry cases.

## Resolution references

- `BODY-001` — 2026-07-27.
