---
module: filter-plugin
title: Plugins have no access to request body bytes
severity: high
envoy_reference: Envoy decodeData() with BufferInstance and StopIterationAndBuffer
---

# Plugins have no access to request body bytes

## Envoy's design (the proven baseline)

Envoy's `decodeData(Buffer::Instance& data, bool end_stream)` is called for every request body
chunk passing through each filter. Filters may read the buffer, modify it, or call
`decoder_callbacks_->addDecodedData(…)` to inject new data. Combined with
`StopIterationAndBuffer`, a filter can fully buffer the request body, inspect it, and either
let it pass or reject the request. This powers WAF body inspection (ModSecurity integration),
request body schema validation, and HMAC/signature verification over body content — all as
first-class filter capabilities.

## Edgion's current design

The `PluginSession` trait (`edgion-gateway/src/plugins/runtime/traits/session.rs`) is the sole
interface through which all HTTP plugins interact with the request. It exposes request headers,
path, query, cookies, context variables, and various write methods, but contains **no method
for reading request body bytes**. There is no `get_request_body()`, no buffering callback, and
no `RequestBodyFilter` trait.

The `request_body_filter` Pingora callback is implemented in
`edgion-gateway/src/routes/http/proxy_http/pg_request_body_filter.rs`, but it is wired exclusively
to the internal request-mirroring implementation (`mirror_body_tee`). It is not exposed to the
plugin runtime at all. No plugin trait or `PluginRuntime` method receives body chunks.

The only partial exception is the HMAC auth plugin, which has a `validate_request_body` config
field — but the plugin explicitly returns a 500 error when this is set, with the comment
"body not supported" (`edgion-gateway/src/plugins/http/hmac_auth/plugin.rs:359`).

## The problem / gap

The gap between what is needed and what is possible:

1. **HMAC body signature verification is broken**: `HmacAuth` already has a user-facing
   config field `validateRequestBody` but it fails at runtime with HTTP 500. Any user relying on
   HMAC signing that covers the body (which is the normal use of HMAC in API security) hits this.

2. **Body-based WAF / input validation**: a plugin that needs to reject requests with invalid
   JSON bodies, oversized payloads beyond a configured threshold, or bodies containing SQL
   injection patterns cannot be written. The `RequestRestriction` plugin only matches on path
   and method, not body content.

3. **Request body signing and transformation**: request-body rewriting (e.g., adding a field to
   a JSON body before forwarding to upstream) is impossible at the plugin layer. Operators needing
   this either route through a sidecar or cannot implement it at all.

4. **Content-type / body consistency checks**: a plugin enforcing that `Content-Type: application/json`
   requests actually carry valid JSON cannot inspect the body to verify this.

## Impact

- The `validate_request_body` feature of `HmacAuth` is already user-visible but inoperable,
  likely causing confusion for users who configure it.
- Any security-sensitive deployment that requires body-based request verification (e.g.,
  webhook signature validation for GitHub/Stripe webhooks) cannot be served by a plugin and
  must use an external auth service via `ForwardAuth` instead, adding an extra network hop.
- The plugin model's claim of covering the "full request lifecycle" is incomplete: the request
  body is a structural gap.

## Suggested direction

Introduce a `RequestBodyFilter` trait parallel to `UpstreamResponseBodyFilter`:

```rust
pub trait RequestBodyFilter: Send + Sync {
    fn name(&self) -> &str;
    fn run_request_body_filter(
        &self,
        body: &mut Option<bytes::Bytes>,
        end_of_stream: bool,
        session: &mut dyn PluginSession,
    ) -> RequestBodyFilterResult;
}
```

Wire it into `PluginRuntime` (a new `request_body_plugins` Vec) and dispatch from
`pg_request_body_filter.rs` after the mirroring step. To prevent unbounded memory usage,
add a configurable `max_body_buffer_size` limit per plugin; a plugin that calls
`session.buffer_request_body()` implies it needs to see the full body and opts into buffering
up to the limit.

The immediate fix for `HmacAuth` is to implement this trait for `HmacAuth` and remove the
`validate_request_body` 500 guard.
