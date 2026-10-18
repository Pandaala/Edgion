---
name: sso-migration-not-worth-it
description: Use when evaluating proposals to migrate DSL Value::Str to a SSO or zero-copy string type; preserves historical candidate analysis and separates representation migration from localized borrowing.
knowledge_type: intentional-design
---

# DSL `Value::Str` SSO / try_unwrap migration — investigated and deferred

**Date investigated**: 2026-05-15
**Status**: Whole-representation migration remains deferred. Localized consumer borrowing is assessed separately.
**Triggering proposal**: `tasks/todo/dsl-value-str-arc-smartstring.md` (migrate `Value::Str(Arc<str>)` → `Value::Str(Arc<SmartString>)` for SSO + zero-copy `into_string`)
**Design spec produced**: `docs/superpowers/specs/2026-05-15-dsl-value-immutable-string-design.md` (historical path, absent from the current tree; **do not adopt as-is**)

## Current scope and historical evidence

Multiple candidate libraries (`smartstring`, `compact_str`, `ecow`, `kstring`) and one bespoke-type option (`Arc<Box<str>>`, custom `enum { Inline, Heap(Arc<Box<str>>) }`) were evaluated. `kstring` with `default-features = false, features = ["std", "arc", "max_inline"]` was selected and a full design spec written. **An independent subagent review then confirmed by reading kstring 2.0.2 source that the headline benefit — zero-copy `into_string()` for sole-owner heap values — does not exist** (`KString::into_string` always routes through `into_boxed_str` which `memcpy`s; the `ArcStr` backend exposes no consuming method).

The candidate-library observations below describe the May 2026 investigation;
versions, maintenance, security status and toolchain requirements were not
revalidated in this source audit. They are not current dependency advice.

Keep the current `Value::Str(Arc<str>)` representation pending a separately
reviewed migration. Current `Value::try_into_string` copies nonempty Str payloads
into a String, but that does not establish that every caller needs owned text.
Read-only consumer borrowing can avoid the conversion without changing Value's
representation. No workload profile here establishes its throughput benefit or
that DSL allocations are negligible. The historical allocation estimates below
cannot establish either conclusion.

The scoped follow-up is
`task/review-data-panel/dsl-01-string-consumer-ownership.md`. It separates ordinary
argument reads, asynchronous Body byte ownership, and composite URI results.

## When to apply this memo

When reviewing or seeing proposals of the form:

- "`Value::Str(Arc<str>)` should be migrated to `Arc<SmartString>` / `Arc<CompactString>` / `KString` / `EcoString` for SSO"
- "`Value::into_string()` should be zero-copy via `try_unwrap`"
- "We should adopt `smartstring` / `compact_str` / `kstring` in the DSL runtime for performance"
- "`Value::List(Vec<Arc<str>>)` should use an SSO type"
- "EdgionDSL string allocation is a hot path"

Read this historical analysis before repeating the library survey. Verify its premises against current code and the proposed consumer; it does not bar local borrow-based optimizations.

## Why each candidate was rejected

### `Arc<SmartString>` (the original task proposal)

- The outer `Arc` allocates one heap block to hold the `SmartString` struct. Short strings stored inline still cost that allocation, so the **promised "SSO ≤23 B zero allocation" never materializes**.
- `smartstring`'s internal `unsafe` aliases `String`'s memory representation, which is not a stable Rust ABI — works in practice, but soundness rests on an undocumented standard-library invariant.
- Net realized benefit if adopted: zero-copy `into_string` for sole-owner heap values — which would have been a real (small) win versus today.

### `Arc<CompactString>`

- Same SSO-defeating outer-`Arc` problem as `Arc<SmartString>`.
- `compact_str` itself is healthier (no String-repr aliasing, miri/loom tested) but offers no architectural advantage here.
- Net realized benefit if adopted: same as `Arc<SmartString>` — zero-copy `into_string` for sole-owner heap values.

### `EcoString` (no outer Arc; internalizes refcount)

- 15-byte inline threshold (because `EcoString` is 16 bytes total, holding tag + inline data or pointer). Common HTTP middle-length tokens (`application/json` 16 B, `text/html`? 9 B inlines fine, `max-age=3600` 12 B inlines fine, but **`application/json` does not**) fall through to the heap path.
- Whether this threshold matters depends on a measured value distribution; none is supplied here.

### `kstring::KString` with `arc` + `max_inline` (selected)

- 22-byte inline threshold (covers `application/json` and most HTTP tokens).
- `arc` feature gives `Clone` O(1) via `Arc<str>` refcount in heap mode.
- `default-features = false` removes 9 `unsafe` blocks while preserving struct size and SSO capacity — confirmed by reading `string.rs` (the `unsafe` feature is orthogonal to `max_inline`; `static_assertions::assert_eq_size!` enforces the size contract internally).
- Maintainer: Ed Page (Rust libs-team, clap maintainer); license MIT/Apache-2.0; no RUSTSEC against kstring itself.
- Maintenance state: feature-complete; no feature release since 2024-07-25 (~22 months as of investigation date); Renovate auto-bumps Rust toolchain monthly.

### Why `kstring` was still finally rejected (the key finding)

Reading `kstring` 2.0.2 source (`src/string.rs`, `src/backend.rs`):

```rust
// src/string.rs — public API
pub fn into_string(self) -> StdString {
    String::from(self.into_boxed_str())
}

// Inner safe-mode dispatch
match self {
    Self::Singleton(s) => crate::backend::BoxedStr::from(s),
    Self::Inline(s)    => crate::backend::BoxedStr::from(s.as_str()),
    Self::Owned(s)     => crate::backend::BoxedStr::from(s.as_str()),  // memcpy
}
```

`BoxedStr::from(&str)` is `<Box<str> as From<&str>>::from` — always allocates and copies. The `HeapStr` trait that `ArcStr` implements (`src/backend.rs`) **exposes only `as_str(&self) -> &str`** — no consuming method to extract the inner `Arc<str>` for `Arc::try_unwrap`. Even when the runtime is the sole owner of an `Owned` (heap) `KString`, `into_string()` performs exactly one allocation + memcpy, identical to today's `Arc<str>::to_string()` baseline.

**Historical result**: the investigated kstring conversion did not deliver the
proposed zero-copy extraction. The recorded 5–15 saved allocations, hundreds of
gateway allocations, MSRV lift and roughly 98 changed call sites were estimates
or proposal-specific observations, not current measured costs or acceptance gates.

## Alternatives that *could* deliver zero-copy `into_string` and why they were not pursued

### `Arc<Box<str>>`

`Box<str>` is `Sized`, so `Arc::try_unwrap` works, and `String::from(Box<str>)` is zero-copy. Trivial diff (1 line type change + accessor adjustments). But:

- Each string costs **2 heap allocations on construction** (Arc inner block, plus the `Box<str>`'s separately-allocated content), versus today's 1-allocation `Arc<str>` (fat-Arc).
- The relative construction and conversion frequencies were not measured here; no net performance result follows from this representation accounting alone.
- Could be revisited if profiling confirms `into_string()` is in the top allocation sites.

### Custom `enum { Inline([u8; 22], u8), Heap(Arc<Box<str>>) }`

Hand-rolled type, ~80 lines of safe Rust, no new dependency, both SSO and zero-copy `into_string`. This is a historical design candidate, not a selected implementation.

- Long-term maintenance cost (we'd own correctness, benchmarking, fuzz coverage).
- ~1–2 day implementation vs. the 2–3 hour scope originally budgeted.
- Justified only when there is a measured allocation bottleneck.

## Allocation accounting summary (estimated)

The following historical illustrative workload was not measured and is not established as typical. Counts assume the listed operations and sharing patterns; they are not an allocation inventory for current code:

| Path                                    | Current `Arc<str>` | After kstring | After `Arc<Box<str>>` | After custom enum |
|-----------------------------------------|---------------------|---------------|------------------------|--------------------|
| `LoadConst` (cache hit)                 | 0                   | 0             | 0                      | 0                  |
| Short-string construction (≤22 B) × ~15 | 15                  | 0 ⭐          | 30                     | 0 ⭐               |
| Medium/long construction × ~10          | 10                  | 10            | 20                     | 10                 |
| `into_string()` × ~10                   | 10                  | 10            | 0 ⭐ (sole owner heap) | 0 ⭐ (sole owner)  |
| `Add` result × ~3                       | 3                   | 3             | 6                      | 3                  |
| **Total DSL string allocs / request**   | **~38**             | **~23**       | **~56**                | **~13**            |

No measured gateway-wide denominator accompanies this table. It cannot establish that DSL allocations are a small minority, or accept/reject a local optimization. Measure the actual candidate consumer separately from construction and final output ownership.

## Re-evaluation triggers (when to re-open)

Open a new task and revisit the analysis if **any** of the following is observed:

1. **Profiling data** (heaptrack / DHAT / perf record) on a production-shaped Edgion deployment shows `Value::Str` construction or `into_string` paths in the top 10 allocation sites or contributing >5% of allocator time during DSL execution.
2. **The DSL gains a new idiom** that produces large lists or maps of short strings inside tight loops (e.g., a future "iterate every header into a map" builtin used in a per-request loop).
3. **A custom string crate appears in the Rust ecosystem** that delivers SSO + zero-copy `Arc::try_unwrap` semantics without the trade-offs catalogued above (e.g., a stdlib-adopted `Rc<str>` extension, or a stable thin-arc-of-str pattern).
4. **`kstring` or `compact_str` or equivalent gains a true consuming `into_inner` method** on their internal Arc-backed storage, enabling zero-copy `into_string`.
5. **The MSRV concern resolves naturally** (Edgion's stable Rust crosses 1.95+ and stays there) AND one of the above also fires.

## Current source references and historical artifacts

- `edgion-dsl/src/value.rs` — `Value::Str`, `try_into_string` and the distinct nil-friendly `try_as_str` coercion.
- `edgion-gateway/src/plugins/http/dsl/lang/vm.rs` — `RuntimeString`, strict `expect_string`, constant string cache, and Body byte transfers.
- `edgion-gateway/src/plugins/http/dsl/lang/builtins.rs` — argument conversion and checked owned/borrowed result helpers.
- `tasks/todo/dsl-value-str-arc-smartstring.md` and `docs/superpowers/specs/2026-05-15-dsl-value-immutable-string-design.md` — historical proposal/spec paths, absent from the current tree.
- The kstring 2.0.2 excerpts above are retained investigation evidence, not fresh verification of a current release.

## Re-evaluation scope

No runtime representation or dependency changed in this documentation correction.
A future whole-representation proposal needs current source and measurements;
the historical trigger examples are not prerequisites for inspecting localized
borrow-based consumers. Reconstruct necessary design evidence from current
sources rather than requiring unavailable historical task/spec files.
