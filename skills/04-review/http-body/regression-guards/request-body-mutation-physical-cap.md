---
name: request-body-mutation-physical-cap
description: Use when reviewing request-body mutation limits; covers why the immutable gateway physical cap belongs to BodyStorage and how mutation failures interact with plugin policy.
status: fixed
finding_id: BODY-002
closed: 2026-07-27
knowledge_type: regression-guards
---

# BODY-002 — Request-Body Mutation Physical Cap — FIXED

## Conclusion

The gateway-wide physical body cap applies to both downstream capture and every
plugin-created body version. `BodyStorageView` stores the immutable cap and
atomically rejects overwrite or slice replacement before publishing a version
whose final checked length exceeds it.

Fix suggestions that validate independently in each mutating plugin or let each
mutation caller supply its own cap are **not accepted**.

## Core Rationale

**1. The storage layer owns the invariant**

ExtProc, ProxyRewrite, and future body mutators all publish through
`BodyStorageView`. Storing `max_body_size_bytes` in the view's shared inner
state prevents a new caller from forgetting the check or temporarily widening
the cap.

**2. Mutation publication is atomic**

The candidate `MutationBody` computes its length with checked addition and is
compared with the cap before the mutation lock is updated. On failure, later
plugins and all upstream replay attempts continue to see the previous complete
version. This remains true when the captured source is disk-backed and when a
previous mutation was already committed.

**3. Capture and mutation failures have different recovery states**

Capture exceeding the physical cap leaves no complete forwarding body and
always returns HTTP 413. A rejected mutation preserves the previous complete
version, so the mutating plugin's established failure policy applies. ExtProc
therefore honors `failureModeAllow`; ProxyRewrite retains its fixed fail-close
behavior.

## Fix Suggestions Not Accepted

- Check the size separately in ExtProc and ProxyRewrite — duplicates a
  cross-cutting invariant and leaves future mutators exposed.
- Pass a cap argument to every mutation call — a caller could accidentally
  widen the physical limit.
- Publish the candidate and roll back on failure — briefly exposes an invalid
  current version and complicates concurrent readers.

## Re-evaluation Triggers

Re-open this decision only if:

- Body versions can be generated outside `BodyStorageView`.
- The gateway introduces a separately configured output/mutation cap with
  semantics distinct from the physical request-body cap.
- Mutations become concurrent instead of phase-ordered and serial.

## Reference Cases

- Origin finding:
  `tasks/review-body-buffering-runtime/body-002-mutation-physical-cap.md`
- Implementation: `edgion-gateway/src/request_body/storage.rs`
- Context binding: `edgion-gateway/src/plugins/runtime/session_adapter.rs`
- Plugin policy: `edgion-gateway/src/plugins/http/ext_proc/plugin.rs`

## Resolution references

- `BODY-002` — 2026-07-27.
