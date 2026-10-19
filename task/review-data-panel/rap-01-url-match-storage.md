# Decide RequestAccessPolicy URL match-result storage

Status: deferred for user decision. Scope: HTTP RAP profile evaluation.

## Evidence

`RuntimeUrlIndex::lookup` in
`edgion-gateway/src/plugins/http/request_access_policy/plugin.rs` starts with
`vec![false; self.candidates.len()]`, then marks candidate positions selected by
radix exact/prefix matching and regex matching. Rust Vec<bool> stores boolean
elements; this is not a packed bitset. Nonempty candidate profiles create and
initialize this vector for each evaluation, including misses.

`RuntimeProfile::allows` computes the vector once before group evaluation. The
result remains alive across supplemental-condition awaits. Later rules consult
it in their own candidate order, combined with Host and condition predicates.
An earlier primary IP rule can settle a group or deny the profile after this
lookup has already occurred. These are observed costs, not measured hotspots.

The canonical `PathRadixTree::match_all_ext` in
`edgion-algo/src/matcher/radix_path/frozen.rs` separately uses SmallVec capacities
8 for results and 16 for traversal state, with possible heap spill. Changing the
RAP result vector does not remove those costs. The completed per-rule candidate
Range change removes cold index arrays, not this request-local match vector.

The URL reference validation maximum of 10,000 rules is per reference. Controller
`handlers/plugins/request_access_policy_data.rs` checks each referenced list
before extending the aggregate candidates; Gateway compilation concatenates
candidates across rules and groups. Do not treat that limit as a profile-wide
bound or silently truncate the vector at it.

## Decision

Choose whether measured workloads justify changing the current dense vector:

- Inline small storage can avoid a small-profile heap allocation, but increases
  live stack/future storage and still spills for larger profiles.
- Packed bits reduce dense storage and initialization bytes, but add indexing,
  masking and boundary work; use a reviewed representation, not unchecked shifts.
- Sparse candidate indices can reduce storage for low hit density, but introduce
  membership, deduplication and possibly sorting costs. Traversal-result order
  cannot replace policy candidate order.
- Keeping the vector retains simple constant-time membership and compact future
  ownership. Measure this baseline before accepting a more complex shape.

Do not introduce a process-wide scratch pool, cross-request result cache or
consumer-local path matcher under this task. Any such proposal needs separately
reviewed concurrency, memory bounds, cancellation and ownership design.

## Required acceptance

Preserve exact/prefix/regex semantics, multiple paths matching one candidate,
parameterized paths, empty imports, mixed inline/imported candidates and candidate
ownership across AND groups and OR rules. Keep Host checks, supplemental condition
order, short-circuiting, terminal outcomes and shared condition ownership.

Preserve the existing path snapshot before group/async condition evaluation.
Deferring or repeating lookup against live session state is a separate semantic
change; reduced work alone does not establish equivalence. Do not reevaluate
conditions, log them twice or silently use a new reference snapshot.

Benchmark actual aggregate candidate counts, hit density, regex/path overlap,
misses, early primary-IP outcomes and inline/spill thresholds. Include profiles
with no URL candidates, small profiles and large imported profiles. Measure
allocation calls, initialized/retained bytes, CPU work and concurrently suspended
future sizes; a stack allocation can become part of an enclosing heap future.
Report radix scratch and RAP result costs separately. Fewer allocations do not
prove lower concurrent memory or higher throughput.

After user selection, use IMPL with a frozen allow-list, focused RAP/condition
regressions and project checks. Do not change unrelated schema limits or tests
merely to suit the representation. Original constraints include
`skills/04-review/plugin-metadata/mandatory-design/condition-set-single-owner.md`
and `skills/04-review/plugin-runtime/regression-guards/edgion-config-data-incremental-atomic-publication.md`.
This task implements no runtime change and supplies no benchmark result.
