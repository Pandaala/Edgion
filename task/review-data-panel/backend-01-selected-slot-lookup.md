# Remaining RR/CH successful-winner address lookup

Status: selected-slot binary lookup implemented; verification in progress.
The separate RR/CH winner lookup below remains deferred for user decision.

## Selected-slot scope

The user approved binary lookup for ordinary selected-slot resolution and sticky
address recovery. `ResolvedServiceSection::slot_for_addr` now searches the private,
unique numeric socket array; `all_slots()` exposes immutable iteration. Ordinary
selection retains exact Arc validation, while sticky preserves unready discovery
and existing eligibility/fallback behavior. No selector algorithm changes or new
index storage are included.

The construction and identity contract lives in
[discovery geometry ownership](../../skills/04-review/backends/regression-guards/discovery-normalized-geometry-ownership.md#sorted-section-slot-lookup).
Keep this task for the independent RR/CH issue below; completing binary lookup
does not authorize removing that remaining investigation.

## Earlier RR/CH winner resolution (audit at c3ad51cfb)

There is a separate lookup before the section-local binary slot lookup.
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
candidate lookups, CH ring address resolution, or the later section slot lookup.
Do not introduce a per-request heap container, duplicate health protocol, or a
new global map to carry the result. Keeping the existing implementation remains
an option. Evaluate this local alternative against the implemented binary slot lookup
using whole-selection cost, not only hash-lookup counts.

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
alongside all-rejected and fallback selection. This RR/CH appendix records
historical source inspection at c3ad51cfb, not fresh verification of that separate
optimization. The selected-slot implementation and its verification are tracked
in the section above and the owning knowledge page.
