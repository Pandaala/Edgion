# D0002 — Make EndpointSlice Ownership Transitions Generation-safe

## Meta

- Status: needs-discussion
- Severity: high
- Discovered in: B0037, `edgion-gateway/src/backends/discovery/endpoint_slice/`
- Category: correctness

## Evidence

`EpSliceStore` maintains coupled forward and reverse indexes from an EndpointSlice key to its owning Service and from a Service to its EndpointSlice keys. A same-key `add` overwrites the forward entry without removing the key from its previous Service bucket. Compressed delete-to-recreate events can therefore leave the replacement slice visible through both the old and new Service. The affected-Service set also omits the old owner on this path, so its selector/runtime generation is not refreshed.

An ordinary update that changes the Service label reports both owners, but last-slice cleanup is gated on an explicit remove count. The old Service can consequently retain an empty selector cell or runtime state after an update-only ownership migration.

## Why deferred

A safe fix requires one ownership-transition contract across compressed event classification, store upsert behavior, affected-Service reporting, selector publication, and backend draining/removal. Fixing only the reverse index or only handler cleanup leaves the other stale-state path intact.

## Impact and open questions

- A recreated EndpointSlice can publish its replacement endpoints through the previous Service.
- A relabeled final slice can leave stale per-Service selector/runtime state.
- Define whether every `add` is an idempotent upsert and how the prior owner is surfaced.
- Define cleanup from resulting ownership state rather than from raw event-list remove counts.
- Preserve atomic forward/reverse index publication and existing full-set behavior.

## Candidate next step

Design and test one generation-safe transition matrix covering new add, same-owner replacement, same-key owner replacement, label-changing update, explicit remove, and compressed delete-to-recreate. Assert both store indexes, affected-Service sets, selector publication, and old runtime cleanup before implementation.
