---
name: build-lock-single-mutex-intentional
description: Historical rationale for the retired ServiceSelectorCell build_lock design; use only to understand why its coarse lock was accepted before eager immutable selector generations replaced that model.
knowledge_type: superseded
---

# lb-F010 — historical single `build_lock` tradeoff — SUPERSEDED

**Status:** Superseded · the reviewed lazy selector model is inactive
**Original decision:** Accepted tradeoff · 2026-05-24 · no code change

## Current conclusion

This decision no longer describes active Gateway behavior. The former
`ServiceSelectorCell` model lazily built per-policy selector slots behind one
`build_lock`. Current discovery publication instead eagerly builds all four
`CompiledSelectors` from one immutable ready-backend snapshot and publishes them
inside the joint `ResolvedServiceSection` generation. Request selection reads an
already-built selector and does not acquire a selector build lock.

Do not use the old rejection of per-slot locks as a current rule or as evidence
that active selector construction serializes requests. Review current behavior
against the immutable discovery-generation design in
`skills/01-architecture/02-gateway/07-backends.md` and the implementations in
`edgion-gateway/src/lb/compiled_selector.rs` and
`edgion-gateway/src/backends/discovery/coordinator.rs`.

## Historical rationale

Under the retired design, one coarse mutex covered lazy construction of the RR,
consistent-hash, least-connection, and EWMA slots. Concurrent first builds for
different policies could serialize, but steady-state reads used lock-free slot
loads. The contention was therefore bounded to rare first-use work.

The mutex also coordinated three operations as one invariant:

- `get_or_build` compiled a missing slot from the current backend snapshot.
- `replace_backends` replaced the snapshot and cleared all selector slots.
- `hydrate_if_empty` performed its empty-check and publication atomically with
  replacement.

At that time, replacing the coarse mutex with per-slot locks would have required
replacement and hydration to acquire every slot lock in a fixed order. That
added a permanent lock-ordering protocol to improve only uncommon warm-up
contention. Rejecting that change was proportionate for the code then under
review.

## Why it was superseded

The successor was not a finer-grained locking change. The joint discovery-
generation redesign removed lazy selector publication entirely:

- `CompiledSelectors::build` eagerly compiles every policy from the same
  immutable `Arc<[Arc<LbBackend>]>`.
- `ResolvedServiceSection` owns those selectors alongside the exact `all_slots`
  and `ready_slots` generation they represent.
- The coordinator publishes the complete section atomically; requests capture
  and select from that immutable generation.

Consequently, the old `build_lock`, its slot-reset coordination, and its
re-evaluation triggers are no longer applicable. Any future concurrency finding
must be evaluated against the current eager publication call chain rather than
reviving the former lazy-cell tradeoff.

## Historical rejected alternatives

- `[Mutex<()>; 4]` per-slot mutexes, because replacement and hydration would
  have needed an all-lock ordering invariant.
- Per-slot `OnceLock` plus a coarse replacement lock, because resetting slots
  still required coordination with in-flight builds and introduced a second
  synchronization mechanism.

These are preserved as historical context, not present-day prohibitions.

## References

- Historical finding: `lb-F010`, 2026-05-24.
- Successor architecture: `skills/01-architecture/02-gateway/07-backends.md`.
- Current selector bundle: `edgion-gateway/src/lb/compiled_selector.rs` —
  `CompiledSelectors::build`.
- Current joint publication: `edgion-gateway/src/backends/discovery/coordinator.rs`
  — `ResolvedServiceSection` construction.
