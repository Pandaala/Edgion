---
name: custom-upstream-reuse-is-independent
description: Review rule for independent downstream and custom-upstream session reuse decisions in the Pingora proxy pump.
status: implemented-locally
finding_id: gw-16-review-03
---

# Custom Upstream Reuse Is an Independent Decision

## Conclusion

The custom-protocol response pump reports downstream-session reuse and upstream-session reuse as
separate outcomes. A completed downstream response does not prove that the custom upstream session
is still reusable, and an upstream failure after a terminal downstream header must not force the
already-complete downstream connection closed.

The proxy returns a custom session through `Connector::release_http_session` only when the custom
pump explicitly marks the upstream reusable. Otherwise it drops the session. The connector
contract therefore requires cleanup needed on non-reuse paths to have a `Drop`-based fallback in
the session implementation.

## Lifecycle Rules

- A normal completed exchange may reuse both downstream and upstream sessions.
- A response completed downstream before a later upstream failure may keep the downstream session
  while dropping the upstream session.
- Header-write failures, body-write failures, terminate paths, and post-terminal failures never
  return the custom upstream session for reuse.
- An application rejection before `write_request_header` leaves the upstream session pristine and
  returns it through `release_http_session`, matching the H1 connector behavior.
- A non-ordinary request-body disposition rejected before the header write has the same pristine
  reuse result; failing the request and destroying the untouched connection are separate decisions.
- `release_http_session` is a reuse hook, not an unconditional discard callback.

## Rejected Fixes

- Folding upstream reuse into downstream reuse loses the valid
  `downstream reusable / upstream not reusable` state.
- Calling `release_http_session` for every outcome makes the connector responsible for guessing
  whether a potentially corrupted session is safe to pool.
- Marking every error non-reusable discards sessions on policy failures that occur before any
  upstream bytes are written.

## Regression Boundary

Tests must cover a pre-write rejection that invokes `release_http_session` without calling the
custom session's header writer, and a post-terminal failure that preserves the downstream response
while proving the upstream release hook was not called.
