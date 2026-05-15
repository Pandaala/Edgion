# DSL `Value::Str` SSO / try_unwrap migration — investigated and deferred

**Date investigated**: 2026-05-15
**Status**: Deferred indefinitely. Re-open only on the conditions listed at the end.
**Triggering proposal**: `tasks/todo/dsl-value-str-arc-smartstring.md` (migrate `Value::Str(Arc<str>)` → `Value::Str(Arc<SmartString>)` for SSO + zero-copy `into_string`)
**Design spec produced**: `docs/superpowers/specs/2026-05-15-dsl-value-immutable-string-design.md` (kept as historical record; **do not adopt as-is**)

## TL;DR

Multiple candidate libraries (`smartstring`, `compact_str`, `ecow`, `kstring`) and one bespoke-type option (`Arc<Box<str>>`, custom `enum { Inline, Heap(Arc<Box<str>>) }`) were evaluated. `kstring` with `default-features = false, features = ["std", "arc", "max_inline"]` was selected and a full design spec written. **An independent subagent review then confirmed by reading kstring 2.0.2 source that the headline benefit — zero-copy `into_string()` for sole-owner heap values — does not exist** (`KString::into_string` always routes through `into_boxed_str` which `memcpy`s; the `ArcStr` backend exposes no consuming method). The only remaining realized benefit is SSO for short-string construction (≤22 B), saving ~5–15 heap allocations per HTTP request out of several hundred total — too small to justify a new dependency (MSRV 1.95.0 floor) + ~98-call-site diff + maintenance-mode dependency risk.

**Decision**: keep `Value::Str(Arc<str>)`. The `into_string()` "1 allocation always" cost noted in the original task file is real but unavoidable in the current Rust ecosystem without trading other costs. The behavior is correct; only performance is suboptimal in a way that has never been profiled as a bottleneck.

## When to apply this memo

When reviewing or seeing proposals of the form:

- "`Value::Str(Arc<str>)` should be migrated to `Arc<SmartString>` / `Arc<CompactString>` / `KString` / `EcoString` for SSO"
- "`Value::into_string()` should be zero-copy via `try_unwrap`"
- "We should adopt `smartstring` / `compact_str` / `kstring` in the DSL runtime for performance"
- "`Value::List(Vec<Arc<str>>)` should use an SSO type"
- "EdgionDSL string allocation is a hot path"

→ stop and read this memo before re-investigating. The technical conclusions below are final unless one of the **Re-evaluation triggers** at the bottom fires.

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
- HTTP gateway value distribution makes the 15 B threshold the practical breaking point.

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

**Result**: the migration's headline benefit (zero-copy `into_string()`) is fictional. The remaining benefit is SSO during construction of short strings — an estimated 5–15 fewer heap allocations per request in a DSL-string-heavy workload, against a per-request total in the hundreds. Not justified by the cost (new dependency, MSRV 1.95.0 lift, ~98 call-site diff, two-commit churn).

## Alternatives that *could* deliver zero-copy `into_string` and why they were not pursued

### `Arc<Box<str>>`

`Box<str>` is `Sized`, so `Arc::try_unwrap` works, and `String::from(Box<str>)` is zero-copy. Trivial diff (1 line type change + accessor adjustments). But:

- Each string costs **2 heap allocations on construction** (Arc inner block, plus the `Box<str>`'s separately-allocated content), versus today's 1-allocation `Arc<str>` (fat-Arc).
- The DSL VM constructs strings far more often than it calls `into_string()`; this trade is a net regression without profile data demonstrating the opposite.
- Could be revisited if profiling confirms `into_string()` is in the top allocation sites.

### Custom `enum { Inline([u8; 22], u8), Heap(Arc<Box<str>>) }`

Hand-rolled type, ~80 lines of safe Rust, no new dependency, both SSO and zero-copy `into_string`. Real best-practice answer if migration were justified.

- Long-term maintenance cost (we'd own correctness, benchmarking, fuzz coverage).
- ~1–2 day implementation vs. the 2–3 hour scope originally budgeted.
- Justified only when there is a measured allocation bottleneck.

## Allocation accounting summary (estimated)

Per a typical EdgionDSL script handling one HTTP request:

| Path                                    | Current `Arc<str>` | After kstring | After `Arc<Box<str>>` | After custom enum |
|-----------------------------------------|---------------------|---------------|------------------------|--------------------|
| `LoadConst` (cache hit)                 | 0                   | 0             | 0                      | 0                  |
| Short-string construction (≤22 B) × ~15 | 15                  | 0 ⭐          | 30                     | 0 ⭐               |
| Medium/long construction × ~10          | 10                  | 10            | 20                     | 10                 |
| `into_string()` × ~10                   | 10                  | 10            | 0 ⭐ (sole owner heap) | 0 ⭐ (sole owner)  |
| `Add` result × ~3                       | 3                   | 3             | 6                      | 3                  |
| **Total DSL string allocs / request**   | **~38**             | **~23**       | **~56**                | **~13**            |

Versus per-request total allocations in a Pingora gateway (~hundreds, from request parsing, upstream client, body buffers, etc.), the DSL string slice is a small minority. Optimizing it without profile evidence is speculative.

## Re-evaluation triggers (when to re-open)

Open a new task and revisit the analysis if **any** of the following is observed:

1. **Profiling data** (heaptrack / DHAT / perf record) on a production-shaped Edgion deployment shows `Value::Str` construction or `into_string` paths in the top 10 allocation sites or contributing >5% of allocator time during DSL execution.
2. **The DSL gains a new idiom** that produces large lists or maps of short strings inside tight loops (e.g., a future "iterate every header into a map" builtin used in a per-request loop).
3. **A custom string crate appears in the Rust ecosystem** that delivers SSO + zero-copy `Arc::try_unwrap` semantics without the trade-offs catalogued above (e.g., a stdlib-adopted `Rc<str>` extension, or a stable thin-arc-of-str pattern).
4. **`kstring` or `compact_str` or equivalent gains a true consuming `into_inner` method** on their internal Arc-backed storage, enabling zero-copy `into_string`.
5. **The MSRV concern resolves naturally** (Edgion's stable Rust crosses 1.95+ and stays there) AND one of the above also fires.

## Source references for fact-checking the above

- `src/core/gateway/plugins/http/dsl/lang/value.rs:13-26` — current `Value` enum with `Str(Arc<str>)` and `List(Vec<Arc<str>>)`.
- `src/core/gateway/plugins/http/dsl/lang/value.rs:74-89` — current `into_string` (always allocates `s.to_string()`).
- `src/core/gateway/plugins/http/dsl/lang/vm.rs:179` — `const_str_cache: Vec<Option<Arc<str>>>` pre-conversion cache.
- `src/core/gateway/plugins/http/dsl/lang/builtins.rs` — Vec<Arc<str>> construction sites: `:100`, `:218`, `:529`, `:558`; `Value::Str(Arc::from(...))` at `:358`, `:361`, `:523`, `:932`, `:947`.
- `tasks/todo/dsl-value-str-arc-smartstring.md` — the original migration proposal (left in place; its conclusions are superseded by this memo).
- `docs/superpowers/specs/2026-05-15-dsl-value-immutable-string-design.md` — the design spec we wrote and then chose not to adopt (kept as record of the analysis).
- `kstring` 2.0.2 source — confirmed `into_string` non-zero-copy via `https://github.com/cobalt-org/kstring/blob/master/src/string.rs` and `src/backend.rs` (`HeapStr` trait has no consuming method).

## What is NOT in scope of this deferral

- No code changes were made. `Value::Str(Arc<str>)` continues to behave exactly as today.
- No new dependency was added to `Cargo.toml`.
- No follow-up tasks were created (per investigator's direction).

Anyone reopening this should start by reading this memo end-to-end, then the design spec, then run the profile that establishes a re-evaluation trigger above before redoing the candidate-library survey.
