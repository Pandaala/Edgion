---
name: response-header-name-validation
description: Use when reviewing RateLimitLocal or RateLimitRedis headerNames validation, Controller/Gateway preparse parity, or framing and hop-by-hop response-header ownership.
knowledge_type: regression-guards
---

# Rate-limit response header names are validated at the resource boundary

## Conclusion and scope

`LimitHeaderNames` is the shared validation owner for the `limit`, `remaining`,
`reset`, and `retryIn` fields used by `RateLimitLocalConfig` and
`RateLimitRedisConfig`. Every configured name must be an HTTP field-name token of
at most 256 bytes, the four names must be unique case-insensitively, and none may
target `Content-Length`, `Proxy-Connection`, or a canonical static hop-by-hop field. Empty strings
retain their established meaning of omission.

Both plugin configs call the same validation method from `check_fields`, so the
Controller's `EdgionPlugins::validate_plugins` reports the same diagnostics that
the Gateway's `compile_plugins` uses to seal the affected runtime fail-closed.
The two runtime header emitters remain intentionally independent as documented in
[`../intentional-design/header-injection-duplication.md`](../intentional-design/header-injection-duplication.md).

## Rationale and evidence

Before `validate-rate-limit-response-header-names`, both configs accepted raw
header-name strings. The allow path passed them to
`PluginSession::set_response_header`, while the local-reply path passed them to
`ResponseHeader::insert_header`; both discarded errors. Invalid tokens therefore
became silently missing headers, case-insensitive duplicates overwrote earlier
rate-limit values, and framing or hop-by-hop names could corrupt response
semantics.

The corrected implementation reuses `gwapi_types::is_valid_header_token` and
`HEADER_NAME_MAX_LEN`. The canonical static RFC 9110 hop-by-hop set now also
lives in `gwapi_types`; the Gateway forwarding helper re-exports and consumes
that same set instead of maintaining a second list. `Content-Length` is handled
alongside the hop-by-hop set because it is end-to-end but still owned by response
framing.

## Alternatives and limits

- The original rate-limit correction rejected runtime-only filtering because it
  would preserve invalid admission. Static validation remains required. The later
  [shared response mutation boundary](../../plugin-runtime/regression-guards/response-header-mutation-policy.md)
  additionally checks ordinary session writes, including dynamic extension names;
  this is not a replacement for rate-limit configuration validation.
- A rate-limit-local token parser or private reserved-name list was rejected in
  favor of the shared HTTP primitive rules.
- Dynamic names nominated by a message's `Connection` value cannot be known from
  static plugin configuration; forwarding code remains responsible for them.
- `Content-Type` and `Retry-After` are not framing or hop-by-hop fields and are
  outside this guard. Existing configured `retryIn` behavior is unchanged.
- Default `X-RateLimit-*` names, Local-versus-Redis reset-header differences,
  response ordering, local JSON body framing, gRPC handling, and failure policies
  are unchanged.

## Verification and re-evaluation

Verified on 2026-09-15 with focused tests covering valid and omitted names,
invalid tokens, the 256-byte boundary, case-insensitive duplicates, every static
hop-by-hop name, `Content-Length`, both rate-limit config consumers,
Controller/Gateway preparse error-set parity, and the Gateway hop-by-hop helper.

Re-evaluate if rate-limit headers become a typed compiled representation, the
canonical HTTP header-name boundary moves out of `gwapi_types`, or a new response
transport requires a different field-name or framing contract.

## Resolution references

- `validate-rate-limit-response-header-names` — fixed 2026-09-15; source task was
  `tasks/todo/validate-rate-limit-response-header-names.md`.
- `unify-response-header-mutation-policy` — shared response classifier also rejects
  `Proxy-Connection`; current verification is recorded in the shared policy page.
