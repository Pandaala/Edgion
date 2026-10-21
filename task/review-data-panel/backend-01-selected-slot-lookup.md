# Service selected-slot lookup cost

Status: deferred for user decision. Source baseline: `165711990`.

## Evidence and scope

`edgion-gateway/src/backends/selector.rs::slot_for_backend` scans the captured
section's `all_slots` with `Arc::ptr_eq` after a successful selector result.
It retrieves readiness and the slot proof; the selected backend supplies the
serving port. This adds up to N pointer comparisons per successful selection,
even when the selector itself finds a healthy candidate quickly.

`CapturedServiceSelection::endpoint_for_addr` separately scans `all_slots` for
the first matching socket. `backends/mod.rs::take_sp_target_or_select` uses it
for sticky recovery. Unready slots remain discoverable but ineligible, and
missing or unhealthy targets preserve the existing sticky fallback contract.

`discovery/coordinator.rs::materialize_section` creates each backend once and
builds all four compiled selectors from the same backend Arcs in `ready_slots`.
The section includes unready slots in `all_slots`. `normalize_section` currently
produces unique sockets in BTreeMap order and materialization preserves order.
That offers a possible search alternative, but sortedness is not expressed by
the public `all_slots` field's type or a dedicated lookup API.

RR/CH structures are already compiled on publication. LC/EWMA use live atomic
metrics and P2C; this proposal concerns metadata lookup after selection, not
changing selection algorithms, health evaluation or accepted expiry locks.

## Decision and alternatives

Choose whether to investigate reducing this scan, or retain it until profiling
shows material cost. No cardinality distribution or latency benchmark currently
establishes a worthwhile end-to-end gain.

- A section-owned prebuilt index trades additional generation memory and
  publication work for lookup speed. Reuse existing representations where their
  semantics match; do not create a global mutable lookup or duplicate every
  selector's address map. Ordinary selection must still verify exact Arc identity;
  sticky lookup has socket semantics and must include unready slots.
- An encapsulated sorted-slot lookup could exploit current numeric socket order
  without a second map. First establish and test the ordering/uniqueness contract
  across every constructor and retain ordinary-path pointer validation. Do not
  silently substitute address equality for owner identity.
- Returning a section-local slot/index from the selector avoids reverse lookup,
  but couples selector APIs to discovery ownership and requires broader review.
  Any index must remain paired with the generation that owns it.
- Retaining the scan avoids extra memory and API/invariant changes; small sections
  may favor it. Source-level complexity alone does not settle that tradeoff.

## Acceptance for a future implementation

Compare lookup and whole-selection cost over representative small and large
sections, including misses and late matches, ready/unready mixes and reloads.
Report generation memory and publication cost alongside request latency; do not
infer throughput from comparison counts.

Preserve RR, CH (including empty-key fallback), LeastConn and EWMA winner,
rejection and degrade semantics. Metadata lookup must not re-run a winner's
health predicate. Preserve exact owner identity, serving-port proof and captured
membership; old snapshots must remain valid across member replacement. Sticky
lookup retains absent/unready behavior and current first-match semantics. Test
IPv4/IPv6, heterogeneous serving ports, duplicate-origin normalization, runtime
health changes and old/new generations. Any changed representation needs a
separately reviewed IMPL plan and normal project verification.

Original constraints: `skills/04-review/lb/regression-guards/service-runtime-snapshot-selection.md`,
`skills/04-review/lb/regression-guards/rr-ch-core-shared-resolve-helper.md`, and
`skills/04-review/backends/regression-guards/named-port-resolve-against-originating-slice.md`.
This is source-inspection evidence only; no production change or benchmark was run.

## Earlier RR/CH winner resolution (audit at c3ad51cfb)

There is a separate lookup before the section-local reverse scan above.
`lb/compiled_selector.rs::RrCore::select` and `ChCore::select` call
`backend_passes_health`, which resolves each candidate through the immutable
address map before applying the live health predicate. After selection returns
its borrowed `Backend`, `resolve_backend` looks up the successful address again
and clones the final runtime Arc. The second lookup does not repeat health
validation. CH reuses the fallback RR map; there is no duplicate CH map to remove.

A projected-result selector API could return the already resolved winner, or a
reviewed mutable callback could hand off that reference. Either option needs an
interface/lifetime review and measurement against the current simple APIs.
This could save one successful-winner address lookup; it does not remove failed
candidate lookups, CH ring address resolution, or the later section slot scan.
Do not introduce a per-request heap container, duplicate health protocol, or a
new global map to carry the result. Keeping the existing implementation remains
an option. Compare this local alternative and the broader slot/index alternative
above using whole-selection cost, not only hash-lookup counts.

Preserve the exact resolved Arc and one health evaluation per visited candidate.
The address map is built from the original backend list with last-write-wins for
duplicate Inet sockets; inner RR ordering/truncation and CH ring preparation are
separate. Returning the inner candidate's original owner by position would not
by itself preserve map resolution for duplicate sockets. Canonical Service
normalization supplies unique sockets today, but an API change must verify every
constructor/caller rather than assume that property everywhere. Non-Inet
candidates remain unresolvable. Keep RR cursor/fallback ordering, CH distinct-
backend budgets, empty-key fallback, generation lifetime and live health reads.
The existing shared resolver/helper and shared CH/RR map remain canonical.

The inspected compiled-selector tests cover the returned live RR Arc and an
existing keyless CH result, but do not establish duplicate-socket last-wins or
no-second-health-call coverage. Those are explicit future implementation cases,
alongside all-rejected and fallback selection. This addition is source evidence
only: no runtime change, new tests or benchmark. Fresh formatting, workspace
check/Clippy and documentation validation passed for this documentation batch;
existing unrelated Clippy warnings remain.
