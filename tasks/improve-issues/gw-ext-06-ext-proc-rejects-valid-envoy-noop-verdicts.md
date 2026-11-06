# gw-ext-06: ExtProc treats valid Envoy no-op verdicts (empty `HeadersResponse`/`BodyResponse`, `clear_route_cache`) as bad verdicts

- **Category**: design-error
- **Severity**: P1 (significant)
- **Scope**: local
- **Location**: `edgion-gateway/src/plugins/http/ext_proc/plugin.rs:329-341` (`apply_request_common_response`), `plugin.rs:440-448, 515-523, 815-824` (request-header verdicts), `plugin.rs:578-581, 1000-1008` (response-header verdicts), `plugin.rs:593-649` (`validate_request_common_response`, `validate_response_headers_common`, `has_unsupported_common_fields`), `edgion-gateway/src/plugins/http/ext_proc/streamed.rs:174-181`, `edgion-gateway/src/plugins/http/ext_proc/response_streamed.rs:185`

## Summary
The plugin claims Envoy `ext_proc` v3 wire compatibility. However, it rejects two verdict
shapes that Envoy accepts and that real processors routinely send:
1. A phase response whose `CommonResponse` is absent, for example
   `ProcessingResponse{request_headers: HeadersResponse{}}`. In Envoy this is the canonical
   "continue, no mutation" reply. Edgion returns `bad verdict / missing common response`.
2. `CommonResponse.clear_route_cache = true`. Envoy honors or ignores this flag depending on
   `route_cache_action`, and it is never an error. Edgion counts it in
   `has_unsupported_common_fields` and fails.

Both errors go through `fail_mode`. With the default `failureModeAllow: false` the request
receives **HTTP 500**. With fail-open the processor's verdict is silently skipped.

## Current behavior (evidence)
- Request headers (buffered): `let Some(common) = hr.response.as_ref() else { return
  self.fail_request_owner(.., "bad verdict", Some("missing common response")) }`
  (`plugin.rs:440-448`; the same pattern at `515-523` and `815-824`).
  `apply_request_common_response` does the same for absent `common` (`plugin.rs:329-336`).
- Response headers: `let Some(common) = common else { return self.fail_mode(.., "bad verdict",
  Some("missing common response")) }` (`plugin.rs:578-581`, and `1000-1008` on the attached
  path).
- Streamed request body: `let Some(common) = br.response else { return self.fail("bad
  verdict", Some("missing common response")) }` (`streamed.rs:176-178`).
- `has_unsupported_common_fields` returns true for `common.clear_route_cache`
  (`plugin.rs:643-649`). It is checked in request validation (`plugin.rs:595`), response
  headers (`plugin.rs:633`), streamed request body (`streamed.rs:179`) and streamed response
  body (`response_streamed.rs:185`), and every check fails the phase.
- The user doc (`docs/en/.../ext-proc.md`, Compatibility and Limitations sections) says that
  the proto is "wire-compatible" and lists the unsupported features. It mentions neither
  rejection.

## Industry comparison
- **Envoy** (`source/extensions/filters/http/ext_proc/processor_state.cc`,
  `handleHeadersResponse` / `handleBodyResponse`) applies mutations only
  `if (response.has_response())`. An absent `CommonResponse` is a plain CONTINUE. This is
  the documented minimal reply in the `external_processor.proto` comments, and it is what
  Go/Java SDK processors produce from a default-constructed `HeadersResponse{}`.
- **Envoy `clear_route_cache`**: with `route_cache_action: DEFAULT` the route cache is
  cleared when the flag is set together with header mutations; with `RETAIN` it is ignored;
  with `CLEAR` it is always cleared. Processors such as Envoy AI Gateway's ext_proc router
  set `clear_route_cache: true` on header responses as a matter of course.
- An interoperable host accepts these shapes. Edgion has already selected the route when the
  plugin runs, so the closest Envoy equivalent is `RETAIN`: accept the flag and ignore it,
  optionally with a debug counter.

## Impact
- An off-the-shelf processor that answers "no change" with a default message, which is the
  most common reply on high-volume paths, breaks every request on the route. The default
  configuration turns this into 500s, and the access log shows only `bad verdict`.
- A processor written for Envoy that sets `clear_route_cache` cannot be used at all.
- With `failureModeAllow: true` the same processors are bypassed without notice, which
  weakens any policy they enforce.

## Suggested direction
1. Treat an absent `CommonResponse` in `HeadersResponse` / `BodyResponse` as
   `CommonResponse::default()`, that is, CONTINUE with no mutation. Keep phase-matching
   and `ImmediateResponse` validation unchanged.
2. Accept `clear_route_cache` and ignore it (Envoy `RETAIN` semantics). Document that route
   re-selection after ExtProc is not supported. Keep rejecting `CommonResponse.trailers`,
   which Envoy also does not implement.
3. Add lock-in tests with a default `HeadersResponse{}`, a default `BodyResponse{}`, and
   `clear_route_cache: true` for both buffered and streamed paths.

## Notes
- Checked `skills/04-review/ext-proc/regression-guards/ext-proc-phase-verdict-matching.md`
  (phase/verdict type matching) and `opaque-header-fidelity.md`. Neither establishes that an
  absent `CommonResponse` or `clear_route_cache` must be rejected. The "a buggy processor
  must not bypass fail-close" rationale on `fail_mode` concerns malformed or wrong-phase
  replies. An empty `CommonResponse` is neither: it is valid protocol.
- Existing unit tests build `HeadersResponse::default()` in client tests
  (`edgion-gateway/tests/unit/plugins/http/ext_proc/client_test.rs:342, 602, 609`). They
  exercise the transport only, not plugin verdict handling.

## Verification (independent)

- **Verdict**: CONFIRMED (severity P1 unchanged)
- `edgion-gateway/src/plugins/http/ext_proc/plugin.rs:329-336` and `:441-448`: an absent `CommonResponse` returns `bad verdict / missing common response` through `fail_request_owner`/`fail_mode`.
- `plugin.rs:643-649`: `has_unsupported_common_fields` is true for `clear_route_cache`, and both `validate_request_common_response` (`:595-597`) and `validate_response_headers_common` (`:633-635`) fail on it.
- Envoy applies mutations only when `has_response()`; a default `HeadersResponse{}` is a valid CONTINUE, so this is a wire-interop gap, not malformed input.
- No `skills/04-review/ext-proc/*` page or the user doc records either rejection as intended.

## Re-verification (feature-10-28)

- **Verdict**: STILL-VALID (severity P1 unchanged)
- `ext_proc/plugin.rs`, `streamed.rs` and `response_streamed.rs` are unchanged; the no-op verdict rejection paths are at the cited lines.
- `tests/unit/plugins/http/ext_proc/client_test.rs` changed only for the `tonic_prost::ProstCodec` import, not for verdict acceptance.
- `ext-proc-phase-verdict-matching.md` is unchanged.
