# Decide CSRF response cookie header ownership

Status: deferred for user decision. Scope: CSRF cookie production, not pending
response-header queue execution.

## Evidence

`Csrf::set_csrf_cookie` in `edgion-gateway/src/plugins/http/csrf/plugin.rs`
borrows the configured name and encoded token into CookieBuilder, formats the
cookie into a String, then passes its borrowed text to `append_response_header`.
The canonical session adapter parses that text with HeaderValue::from_str into
an independently owned value before appending or queueing it. The formatted
String is temporary; transferring its storage is a possible optimization.

The existing typed append API can receive an owned HeaderValue. However, storage
transfer may retain formatting spare capacity. Locked bytes 1.12.1 converts a
Vec with spare capacity using an additional Shared allocation; exact-capacity
storage takes a different branch. Cookie 0.18.2 Display does not promise an
exact-capacity String. Fewer copied bytes therefore do not establish fewer
allocations, lower retained memory or improved throughput.

Cargo.lock selects http 1.5.0. Before implementation, verify its exact
HeaderValue construction and clone behavior: the previously inspected http
1.4.2 source is not evidence for the locked version. In particular, do not
assume passing String directly to from_maybe_shared transfers storage; verify
the Bytes path and its validation against the selected dependency.

## Decision

Choose between keeping the current simple path, transferring the cookie String
through owned bytes, or allocating exact-capacity output. Direct formatting
without the cookie library is a separate, more complex alternative requiring
complete output equivalence. Measure allocated bytes, allocation counts,
retained capacity, first shared clone and queue replay costs before choosing.
No runtime change or measured benefit is claimed by this task.

## Required acceptance

- Preserve cookie bytes and attribute order, Path, SameSite, Max-Age, configurable
  Secure and intentionally absent HttpOnly. Preserve token generation, entropy,
  signature, encoding, lifetime and existing PluginLog outcomes.
- Keep Append semantics, repeated Set-Cookie values and per-name order in both
  queued and directly bound response states. Preserve canonical protection and
  the later local-reply replay boundary.
- Preserve malformed-value handling: the string adapter returns a successful
  no-op without a bound response, but returns an error when a response is bound.
  Constructing a typed value earlier must not silently change these outcomes.
- Distinguish normal validated configuration from bypass construction. Validation
  requires a nonempty HTTP-token name of at most 256 bytes, excludes `cookie`
  case-insensitively and requires a positive lifetime; Csrf::new does not itself
  run that validation. Exercise bypass cases explicitly without claiming they
  represent accepted operator configuration.
- Compare valid name/lifetime boundaries and both Secure values against the
  cookie library. Test direct, queued and local replacement behavior with
  pre-existing cookies. Never expose generated tokens or cookie values in
  diagnostics, benchmarks or new logging.
- Keep shared session semantics unified; do not introduce a CSRF-specific queue,
  relaxed parser, cache or request-path tracing to avoid the ownership decision.

Related but separate: [pending response-header execution ownership](response-01-pending-header-ownership.md)
concerns cloning retained operations during application, not producer formatting.
Original constraints: `skills/04-review/csrf/regression-guards/set-cookie-append.md`,
`skills/04-review/csrf/regression-guards/cookie-secure-configurable.md`,
`skills/04-review/plugin-runtime/regression-guards/response-header-mutation-policy.md`
and `skills/04-review/ext-proc/regression-guards/opaque-header-fidelity.md`.
