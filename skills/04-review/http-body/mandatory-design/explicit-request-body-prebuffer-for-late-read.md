---
name: explicit-request-body-prebuffer-for-late-read
description: Use when reviewing or changing response-header-stage access to the proxied request body, explicit RequestBodyBuffer behavior, or request-body cache lifetime.
knowledge_type: mandatory-design
---

# Explicit Request-Body Prebuffering for Late Reads

## Decision

`RequestBodyBuffer` is the canonical opt-in mechanism for a
`requestPlugins` entry to capture the complete proxied request body before
upstream forwarding. A later DSL entry in `upstreamResponsePlugins` may
read that existing shared cache.

The stage boundary separates capture from access:

- request-stage consumers may initiate bounded capture through the one shared
  `BodyStorage`;
- response-header-stage consumers may only read an existing cache;
- late mutation and late capture are refused;
- a truthfully bodyless request reads as the canonical empty body without
  allocating a cache.

Do not add static rejection for response-stage `req.body`: whether a cache
exists is request-scoped runtime state under explicit operator control.

## Lifetime boundary

Keep the read-only `BodyStorageView` through all final response-header plugin
layers: the synchronous `upstream_response_filter` chain and then the async
`response_filter` chain, each across route, backend, Gateway, then global.
Release it immediately after the async chain (`pg_response_filter.rs`), the
last late request-body consumer since DSL moved to `upstreamResponsePlugins`
(dsl-08). Interim 1xx responses and status-retry decisions return before the
release point so the final response or retry can still use the cache.

This release point is required for streaming responses. Retaining the view
until request teardown would let SSE and large downloads hold spilled-body
file descriptors or materialized bytes for the entire response lifetime.
Framing metadata for mutated request replay is separate and must remain
available for retries.

## Conditions and failure policy

`RequestBodyBuffer` is a non-security capture primitive, so its intrinsic body
requirement contains an empty condition set. This lets an operator narrow
capture with `body.conditions`. Its read-failure policy remains fixed
fail-close; an operator cannot weaken it to fail-open.

Security consumers such as HMAC keep their unconditional intrinsic
requirements. Do not generalize the condition-preservation exception in a way
that lets operators narrow an authentication or validation plugin's
enforcement scope.

If a response-stage DSL read finds a request that declared a body but no cache,
the VM raises internal `BodyAccess` and returns the fixed generic HTTP 500
response regardless of DSL `errorPolicy`. This missing-cache boundary is not a
configurable failure mode. A bodyless request still resolves to canonical
empty storage.

Body mutation methods are statically valid only in `requestPlugins`.
Configuration validation and the response-stage factory both reject a script
that attempts late mutation; runtime state is not used to reinterpret it as a
read-only no-op.

## Verification

Keep both integration directions:

- explicit prebuffer followed by response-stage DSL `req.body` succeeds;
- a declared body without prebuffer fails at the late read and never starts
  capture.

Unit coverage must also pin bodyless late reads, late-mutation refusal,
operator condition preservation, and release after the final response-header
plugin chain.

## Resolution references

- `BODY-WORK-007` — 2026-07-27.
