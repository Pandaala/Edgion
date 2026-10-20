# Evaluate the Route hostname ID-to-content representation

Status: deferred for user decision. Priority: P3. Type: performance investigation.
Inspected baseline: `be293e8ea`.

## Evidence and decision

`edgion-algo/src/matcher/radix_hostname/mod.rs` serves HTTPRoute, GRPCRoute and
TLSRoute through one canonical RadixHostnameMatcher. Its frozen tree assigns
dense, private HostPatternId values by pattern enumeration. matching_values maps
each yielded ID through content(), which looks up HashTrieMapSync and returns
borrowed contents. This does not allocate or clone an Arc on a query.
The locked rpds 1.2.1 get hashes the ID and traverses its persistent map.

The map also owns an important update contract: cloning a matcher shares its
content structure; content-only with_changes patches paths while keeping the
tree. with_groups explicitly scans all keys but reuses topology and unchanged
payload Arcs. A changed key set rebuilds IDs and remaps by canonical pattern.
Readers of old snapshots must retain the corresponding old contents.

Dense IDs suggest evaluating indexed immutable or persistent sequence storage.
A flat array can avoid hashing but may copy the full handle array and clone many
Arcs for a small content patch. A persistent indexed representation has its own
lookup depth, node layout and update costs. Neither is established as superior.
Compare concrete designs with the current map; do not assume a new dependency,
second read cache or duplicate content owner is necessary.

The user decision is whether realistic lookup savings justify any construction,
update or retained-generation cost. Keeping the current representation is a
valid outcome. This is not a correctness finding or a measured performance
regression. The delivered wildcard-ancestor links optimize tree traversal and
do not establish that changing content storage is worthwhile.

## Acceptance for later investigation and implementation

- Preserve complete same-generation topology/content pairing and private IDs.
  Missing contents remain internal corruption, never a silent no-match that
  allows a weaker route candidate.
- Preserve exact-first and wildcard continuation order, laziness, borrowed
  results, query allocation/lock/Arc-clone freedom and non-Clone payload support.
  Add no protocol, Listener or certificate policy to the leaf matcher.
- Preserve coalesced final-occupancy semantics, same-key topology sharing,
  unchanged payload identity, correct remapping after topology changes, and
  independent old/new snapshot results through deletion and reinsertion.
- Measure query hit/miss/continuation, single and batched content patches,
  same-key full replacement, topology changes, many Listener snapshots, and
  retained old generations. Report lookup latency alongside build/update cost,
  allocation count, requested/live/peak bytes and final-reader reclamation.
- Start from `edgion-algo/tests/benches/radix_hostname.rs`, which includes the
  production ID-to-content lookup and held-generation scenarios. Retain its
  semantic assertions. Its counting allocator measures requested bytes, not RSS;
  batch means are not latency percentiles. Record machine, profile, input shape,
  command and selected baseline; historical tree-layout results are not evidence
  for a new content representation.
- Use an independently reviewed implementation scope and run the canonical
  matcher oracle, layout/ownership/update tests, all three Gateway route adapters
  and project checks. Preserve existing fixture-failure visibility.

The current ownership and complexity contract is in
[Route hostname matching](../../skills/12-algorithms/route-hostname-matching.md).
No candidate implementation or comparative measurement has been performed here.
