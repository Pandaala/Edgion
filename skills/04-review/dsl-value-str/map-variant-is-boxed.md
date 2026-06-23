---
name: map-variant-is-boxed
description: Use when reviewing DSL Value enum size or Map variant findings; documents that Value::Map is intentionally Box-wrapped to enforce the size invariant and unboxing is not accepted.
---

# DSL `Value::Map` is intentionally Boxed — do not unbox

**Date applied**: 2026-05-15
**Status**: Implemented. `edgion-gateway/src/plugins/http/dsl/lang/value.rs` defines
`Map(Box<IndexMap<String, Value>>)`; the size invariant is locked by the
`assert_value_size_invariant` unit test in the same file.
**Triggering analysis**: `tasks/todo/dsl-vm-rhai-research/01-value-size-check.md` (deleted after fix).

## TL;DR

`IndexMap<String, Value>` is 72 B on a 64-bit target (hashbrown HashTable + entries
Vec + RandomState). If `Value::Map` carries it inline, the whole `Value` enum
becomes 72 B and `VmState`'s `SmallVec<[Value; 16]>` for `stack` and `locals`
each occupies ~1160 B inline (~2.3 KB total per request). Boxing the IndexMap
shrinks `Value` to 24 B and the per-request VmState stack frame to ~0.83 KB.

**Decision**: keep `Map(Box<IndexMap<String, Value>>)`. Do not propose unboxing.

## When to apply this memo

When reviewing or seeing proposals of the form:

- "`Value::Map` has an unnecessary `Box` — remove it / inline the IndexMap"
- "Boxing `IndexMap` adds an extra heap allocation per Map construction; just inline it"
- "`Box<IndexMap>` looks weird, IndexMap is already heap-backed"
- "Why is only `Map` Boxed but not `List(Vec<Arc<str>>)` / `Str(Arc<str>)`?"

→ stop and read this memo before re-investigating. Do not accept the suggestion.

## Why Box only `Map`

- `size_of::<Value>()` is determined by the largest variant. With `Map(IndexMap)` inline, that's 72 B; with `Map(Box<IndexMap>)`, the largest variant becomes `List(Vec<Arc<str>>)` at 24 B.
- `List`, `Str(Arc<str>)`, `Int(i64)`, `Bool(bool)`, `Nil` are all ≤ 24 B already. Boxing them gives no further size reduction but adds one heap allocation per construction. Net negative.
- The extra `Box::new` allocation per `Value::Map` construction is amortized by the IndexMap's own existing entries-vec / hash-table allocations (which are unaffected). One thin-pointer alloc per Map is invisible under jemalloc/mimalloc.
- Read paths use `Box<T>`'s `Deref`/`DerefMut` and require no source change for `m.get(...)`, `m.len()`, `m.iter()`, `&m == &m`, etc. The one exception is `for (k, v) in &boxed_map` — `&Box<IndexMap>` does not implement `IntoIterator`; use `boxed_map.iter()` instead.
- `Box<T>: Clone` defers to `T: Clone`, so deep-clone semantics for the inner IndexMap are preserved. No behavior change vs. the inline form.

## When to revisit

Only re-open the layout decision if one of the following occurs:

1. A new `Value` variant is added that pushes `size_of::<Value>()` past 24 B (e.g. `Bytes(Vec<u8>)` or `Regex(Regex)` inline). The `assert_value_size_invariant` test will fire — at that point evaluate boxing the new variant first, not unboxing `Map`.
2. `IndexMap` upgrades change its inline size dramatically (e.g. drops to 24 B or below). The size-invariant test will catch it; only then is unboxing worth re-evaluating.
3. EdgionDSL gains closures or shared mutable values, requiring a `Shared(Arc<Locked<Value>>)` variant — the entire size accounting is then redone.
4. A profiler shows DSL Map-construction's `Box::new` alloc as a measurable hot-path cost (currently zero evidence of this).

## Cross-references

- `tasks/todo/dsl-vm-rhai-research/01-value-size-check.md` — full numerical analysis, Rhai comparison, route map (deleted upon implementation).
- `[[sso-migration-not-worth-it]]` — sibling memo for why `Value::Str(Arc<str>)` is **not** migrated to an SSO type.
- `tasks/todo/dsl-vm-learn-from-rhai.md` — broader Rhai-comparison todo (this fix closed the "Value size" item P1).
