---
name: request-mirror-committed-body-snapshot
description: Use when reviewing RequestMirror after an earlier request plugin mutates the body; covers immutable snapshots, cursor isolation, framing, and discarded mirror responses.
status: fixed
finding_id: BODY-014
closed: 2026-07-27
---

# BODY-014 — RequestMirror Uses the Committed Body Snapshot — FIXED

## Conclusion

When an earlier request plugin has created a canonical `BodyStorageView`,
RequestMirror clones the current immutable committed bytes into its detached
task. The mirror never consumes Pingora's upstream replay cursor. With no body
view, RequestMirror retains its original streaming tee behavior.

The mirror response is semantically irrelevant and is dropped after response
headers arrive; it is never aggregated.

## Core Rationale

**1. The mirror is a later logical body owner**

Request plugins run serially. A mutation committed before RequestMirror must be
visible to both RequestMirror and every upstream attempt. Reading the current
view produces that version without retaining a mutable plugin owner.

**2. Snapshot and replay cursors remain independent**

The detached mirror task owns a `Bytes` snapshot. It never calls replay
`rewind`, `peek`, or `consume`, so mirror progress, timeout, and failure cannot
move or block the main upstream cursor. Pingora retries continue to rewind and
replay their own owner.

**3. Each outbound hop owns its framing**

The snapshot path removes inherited `Content-Length` and `Transfer-Encoding`.
`reqwest::Body::from(Bytes)` derives framing from the committed bytes instead
of forwarding metadata that may describe the downstream V0 body.

**4. The no-view path stays streaming**

RequestMirror alone does not opt a request into full-body caching. When no
canonical body view exists, the existing bounded-channel tee remains in use.

**5. Mirror responses have zero aggregation budget**

The mirrored response never affects the client or gateway policy. Dropping it
after headers avoids unbounded response-body memory and prevents an infinite
response from retaining a mirror concurrency permit.

## Fix Suggestions Not Accepted

- Share or rewind Pingora's upstream replay cursor from the detached mirror
  task.
- Force every RequestMirror request through `BodyStorage` when no earlier body
  owner exists.
- Preserve downstream `Content-Length` or `Transfer-Encoding` on the snapshot
  hop.
- Aggregate the discarded mirror response without a hard bound.

## Re-evaluation Triggers

Re-open this decision only if:

- RequestMirror gains response-dependent behavior.
- Request-body versions can change after the request plugin chain.
- The mirror transport stops accepting an independently owned immutable body.

## Reference Cases

- Origin finding:
  `tasks/review-body-buffering-runtime/body-014-request-mirror-after-mutation.md`
- Runtime:
  `edgion-gateway/src/plugins/http/request_mirror/plugin.rs`
- Integration coverage:
  `edgion-tests/src/client/suites/edgion_plugins/body_plugins.rs`
- Ordinary streaming regression coverage:
  `edgion-tests/src/client/suites/edgion_plugins/request_mirror/`
