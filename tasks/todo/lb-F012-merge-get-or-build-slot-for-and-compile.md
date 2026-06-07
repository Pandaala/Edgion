# lb-F012: collapse `slot_for` + `compile` into a single per-policy match in `get_or_build`

- **Severity**: Low
- **Dimension**: ExtractFn / Readability (maintainability only, no runtime impact)
- **Location**: `src/core/gateway/lb/compiled_selector.rs` — `get_or_build` (357-374), `slot_for` (376-383), `compile` (386-393), at current HEAD
- **Origin**: promoted from review finding F-012 (`tasks/lb/lb-F012-get-or-build-slot-for-extract-fn.md`, since deleted)

## Problem

`ServiceSelectorCell::get_or_build` first calls the private helper `slot_for` (a `match` over
`PolicyKind` returning one of four `ArcSwapOption` field references), then calls the free
function `compile` (a second, independent `match` over the same enum that builds the selector).
Two separate matches over the same enum mean adding a fifth `PolicyKind` touches both arms plus
several other sites (field declarations, `replace_backends` slot clears, `new` initializer).

## Important correction to the original finding

The original finding claimed the two matches "can silently diverge" / allow "partial-state
commits". This is **not accurate**: both `slot_for` and `compile` are *exhaustive* matches with
no `_ =>` catch-all, so adding a new `PolicyKind` variant breaks **both** at compile time. The
compiler forces both to be updated — there is no compilable partial state. So this is purely a
readability/maintainability nicety, **not** a correctness risk.

## Proposed fix (single-match merge)

Merge `slot_for` and `compile` into one `match` inside `get_or_build`, where each arm yields both
the slot reference and the build closure for that policy:

```rust
let (slot, build): (&ArcSwapOption<CompiledSelector>, fn(Arc<Vec<Arc<EdgionBackend>>>) -> CompiledSelector) =
    match kind {
        PolicyKind::RoundRobin     => (&self.rr,         |b| CompiledSelector::RoundRobin(RrCore::build(b))),
        PolicyKind::ConsistentHash => (&self.ch,         |b| CompiledSelector::ConsistentHash(ChCore::build(b))),
        PolicyKind::LeastConn      => (&self.least_conn, |b| CompiledSelector::LeastConn(MetricCore::build(b))),
        PolicyKind::Ewma           => (&self.ewma,        |b| CompiledSelector::Ewma(MetricCore::build(b))),
    };
```

Non-capturing closures coerce to `fn` pointers. Delete the standalone `slot_for` and `compile`
functions. The slot reference and the build are then bound together per policy, so they cannot
drift, and adding a new policy needs only one new arm in this block. Behavior-preserving.

This is consistent in spirit with lb-F009 (shared `resolve_backend` / `backend_passes_health`
helpers extracted in the same file).

## Priority

Low / optional. Compiler already enforces exhaustiveness, so there is no urgency. Pick up
opportunistically (e.g. the next time a `PolicyKind` variant is added or this file is touched).
