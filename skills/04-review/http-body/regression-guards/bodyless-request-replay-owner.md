---
name: bodyless-request-replay-owner
description: Use when reviewing request-body injection for a downstream request that declared no body; covers the separate finalized replay seam and lazy empty V0 owner.
status: fixed
finding_id: BODY-008
closed: 2026-07-27
knowledge_type: regression-guards
---

# BODY-008 — Bodyless Request Replay Owner — FIXED

## Conclusion

A downstream request that declared no body can still acquire a canonical empty
V0 when a request-stage plugin commits a body mutation. Edgion lazily installs
an already-finalized `BodyStorage` through Pingora's dedicated
`set_bodyless_request_replay_buffer` seam, publishes the mutation through the
shared view, and replays the final version to every upstream attempt.

The ordinary capture API remains unchanged and is still required for requests
that carry a downstream body.

## Core Rationale

**1. Capture and injection are different state transitions**

`set_request_body_buffer` enters `Capturing`; Pingora owns downstream reads and
calls `finish` at EOF. A truly bodyless request has nothing to capture, so its
separate replay seam accepts an application-finalized buffer directly in
`Ready`. Combining the paths would make buffer finalization ownership implicit.

**2. Owner creation is lazy**

A bodyless read or digest does not allocate storage. Edgion installs the empty
owner only when a mutation API needs to publish a version. Bodyless
WebSocket/CONNECT, gRPC, and post-request-stage mutation attempts remain
refused.

**3. One view connects later plugins and upstream replay**

After V1 is published, later request plugins read that version from
`ctx.request_body`; Pingora's owner reads the same mutation during upstream
replay. Retries rewind the owner rather than recapturing or sharing a plugin
cursor.

**4. Existing invariants still apply**

The empty source uses the same immutable physical body cap, atomic mutation
publication, framing synchronization, replay cancellation behavior, and
attempt-lifetime length metadata as a captured body.

## Fix Suggestions Not Accepted

- Relax `set_request_body_buffer` so it silently changes from capture to replay
  semantics for empty requests.
- Treat bodyless mutation as an ExtProc-only byte override outside
  `BodyStorage`.
- Install an empty owner for every bodyless read or digest.
- Pin the Pingora fork by a moving branch instead of an immutable revision.

## Re-evaluation Triggers

Re-open this decision only if:

- Pingora gains a unified typed registration API that preserves explicit
  capture-versus-ready state.
- A request-body mutation can run after upstream transmission starts.
- A body version can be published outside `BodyStorageView`.

## Reference Cases

- Origin finding:
  `tasks/review-body-buffering-runtime/body-008-ext-proc-bodyless-mutation.md`
- Pingora fork seam: commit
  `57328f15aba6cfd5bf1d3a5b1f9a547f5a4d3bc5`
- Edgion owner binding:
  `edgion-gateway/src/plugins/runtime/session_adapter.rs`
- Storage and replay:
  `edgion-gateway/src/request_body/storage.rs`

## Resolution references

- `BODY-008` — 2026-07-27.
