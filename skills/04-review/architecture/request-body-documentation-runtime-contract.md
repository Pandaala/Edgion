---
name: request-body-documentation-runtime-contract
description: Use when reviewing request-body documentation; distinguishes shared request capture from permanently streaming upstream responses.
status: fixed
finding_id: BODY-011
closed: 2026-07-27
---

# Request-Body Documentation Runtime Contract — FIXED

## Conclusion

Documentation must describe the implemented request-only buffering model:
eligible request-stage plugins share bounded, versioned request-body capture,
while the main upstream response always remains streaming.

Fix suggestions that document full upstream-response buffering as current or
planned behavior are **not accepted**.

## Core Rationale

**1. Request and response bodies have different ownership**

The request runtime may lazily capture the proxied request body, spill it to
disk, and expose the current logical version to serial request plugins. DSL
sees an opaque `req.body`: metadata, predicates, hashing, streaming regex, and
definite JSONPath selection operate transparently across memory and spill
storage; bounded `read()` / `read_all()` are the explicit string-copy APIs.
Request-stage mutation methods update the shared logical body transactionally,
while response-stage mutation is rejected. Wasm whole-request-body mode is
selected by a guest exporting
`proxy_on_request_body`; an optional operator `body:` block adds pre-execution
conditions, a per-plugin size policy, and read-failure handling.

**2. Main upstream responses remain streaming**

Response-body plugins receive the current chunk only. They must not accumulate
an unbounded response or promise a whole-response API such as DSL `resp.body`.
Plugin-initiated outbound calls are separate bounded exchanges and do not
change this rule.

**3. Transitional failure guidance is harmful after implementation**

Stale statements that request-body reads always return `Unimplemented` direct
operators toward unnecessary fail-open workarounds. Documentation must instead
explain capture eligibility, physical and per-plugin limits, the cumulative
`maxBodyScanPasses` budget, stage restrictions, and the fixed generic
fail-close `BodyAccess` boundary. `BodyAccess` is an internal classification,
not a fifth configurable failure-policy shape.

## Fix Suggestions Not Accepted

- Preserve the old warning as historical context — user documentation describes
  supported behavior, not the implementation timeline.
- Document `resp.body` as conditionally available — validation rejects it and
  the main response has no whole-body cache.
- Describe the optional Wasm `body:` policy as the capture switch — the
  exported request-body hook selects capture in the current runtime.

## Re-evaluation Triggers

Re-open this decision only if:

- The request-body ownership or mutation model changes.
- Wasm request buffer mode changes how exported hooks and the optional `body:`
  policy select or order capture.
- The architectural rule requiring main upstream responses to stream is
  explicitly replaced.

## Reference Cases

- `BODY-011`
- `AGENTS.md` — HTTP body handling invariants
- `docs/en/user-guide/http-route/filters/edgion-plugins/wasm.md`
- `docs/en/user-guide/http-route/filters/edgion-plugins/dsl/api-reference.md`
