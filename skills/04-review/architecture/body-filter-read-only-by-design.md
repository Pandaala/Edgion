---
name: body-filter-read-only-by-design
description: Use when reviewing plugin body-filter findings that compare Edgion against Envoy's mutable encodeData/decodeData; covers why the response/request body filter is intentionally read-only (bandwidth-throttle only) and why adding body mutation is a feature, not a bug fix.
---

# Plugin body filter is read-only by design (bandwidth-throttle only) — DESIGN-TRADEOFF

## Conclusion

Edgion's `UpstreamResponseBodyFilter` trait deliberately gives plugins a **read-only**
`&Option<bytes::Bytes>` view of each upstream body chunk and lets them return only an
`Option<Duration>` (bandwidth throttle). Plugins cannot insert, drop, or replace body bytes,
and there is no request-body filter exposed to plugins at all. This is an intentional capability
boundary, not a defect in any running system: the only shipping consumer of this phase is
`BandwidthLimit`, and no plugin in the codebase needs body mutation today.

Fix suggestions of the form "change the trait to `body: &mut Option<bytes::Bytes>` + a
`BodyFilterResult` that can drop/replace the chunk" or "add a `RequestBodyFilter` trait backed
by a Pingora `request_body_filter` callback" are **not accepted as a narrow review fix**. Body
mutation is a wide-impact architectural feature — if/when a real body-transform plugin is
greenlit it belongs in `tasks/todo/` via the brainstorming workflow, not a body-filter trait
signature patch closed under the review-issue flow.

## Core Rationale

**1. It is a feature-parity gap with Envoy, not a triggerable defect**

The finding (`tasks/review-envoy/filter-01`) compares against Envoy's mutable
`Buffer::Instance&` in `encodeData()` / `decodeData()`. Envoy filters can compress, rewrite,
transcode, or WAF-inspect bodies at the filter layer. Edgion's body-filter phase was scoped to
**bandwidth throttling only** — see the deliberate trait doc comment in
`edgion-gateway/src/plugins/runtime/traits/upstream_response_body_filter.rs:11-30` ("Plugins can
inspect the chunk and return an optional Duration to throttle ... This trait deliberately omits
the `&mut PluginLog` argument ... Body filters are invoked once per upstream chunk"). The
behaviour is "this class of plugin cannot be written yet", which harms no in-flight request; the
only concrete current limitation is HMAC body validation, which already fails *gracefully*
(`hmac_auth/plugin.rs:361-362` emits `push_err("body not supported")`, not a crash).

**2. The framework already has the mutable handle; the boundary is intentional, not accidental**

Pingora hands `pg_upstream_response_body_filter.rs:14-20` a `body: &mut Option<bytes::Bytes>`.
The plugin runtime *deliberately* narrows it: `PluginRuntime::run_upstream_response_body_plugins`
in `edgion-gateway/src/plugins/runtime/plugin_runtime.rs` takes
`body: &Option<bytes::Bytes>` and forwards `&body` to each
plugin. So the read-only signature is a chosen contract at the runtime boundary, not a place
where a mutable reference was lost by oversight. Widening it would mean a new chunk-rewrite
result type, a max-expanded-chunk cap to bound allocation, and re-validating every plugin in the
chain against in-place body edits on the per-chunk data-plane hot path.

**3. The blast radius is large and the value is speculative**

A real fix touches the public plugin SDK trait surface and fans out across modules:
`upstream_response_body_filter.rs` (trait), `plugin_runtime.rs` (dispatch), `conditional_filter.rs`
+ `gateway_api_filters/extension_ref.rs` (wrapper impls), `bandwidth_limit/plugin.rs` (the one
real impl + its tests), plus the request-body side would need a brand-new `RequestBodyFilter`
trait, a Pingora `request_body_filter` callback delegated through the runtime, and new
`PluginSession` accessors. All of that is per-chunk hot-path code. With zero shipping plugins
needing body mutation, this is a speculative capability expansion whose design (streaming vs
buffered, size caps, sync `block_in_place` vs preparse) must be settled by brainstorming, not
bolted on as a review patch.

## Fix Suggestions Not Accepted

- "Change `run_upstream_response_body_filter` to take `&mut Option<bytes::Bytes>` and return a
  `BodyFilterResult`" — wide public-SDK API change on the data-plane hot path, no current
  consumer; design must come from brainstorming.
- "Add a `RequestBodyFilter` trait backed by a Pingora `request_body_filter` callback" — same
  reasoning; the existing `pg_request_body_filter.rs` callback is internal-mirroring-only by
  design and not a plugin extension point.
- "Make HMAC `validate_request_body` work by buffering the body in the plugin" — that depends on
  the request-body capability above; the current graceful `push_err("body not supported")` is the
  accepted behaviour until the feature is designed.

## Re-evaluation Triggers

Re-open this decision only if:

- A concrete body-transform plugin is greenlit (WAF body redaction, response JSON rewrite,
  decompression/re-encryption, data masking, charset transcoding) — then promote to `tasks/todo/`
  and design the body-mutation API via brainstorming.
- HMAC request-body validation is prioritized as a real feature (not just a parity gap).
- The framework gains a non-plugin body-transform need that would justify the runtime contract
  change on its own.

## Reference Cases

- `tasks/review-envoy/filter-01-no-body-modification.md` (closed 2026-05-29, ⚠️ accepted tradeoff)
- Companion finding `tasks/review-envoy/filter-05-no-request-body-access.md` (request-body side of
  the same boundary)
- `edgion-gateway/src/plugins/runtime/traits/upstream_response_body_filter.rs`
- `edgion-gateway/src/routes/http/proxy_http/pg_upstream_response_body_filter.rs`
- `edgion-gateway/src/plugins/runtime/plugin_runtime.rs::PluginRuntime::run_upstream_response_body_plugins`
- `edgion-gateway/src/plugins/http/hmac_auth/plugin.rs:361-362`
</content>
</invoke>
