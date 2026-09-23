---
name: allocator-feature-surface
description: Use when reviewing allocator Cargo features in the Gateway, Controller, or CLI manifests.
knowledge_type: regression-guards
---

# Allocator Features Select the Dependency and Binary Allocator Together

## Conclusion and scope

The `edgion-gateway`, `edgion-controller`, and `edgion-cli` packages expose
`allocator-jemalloc`, `allocator-mimalloc`, and `allocator-system` as their
allocator selection features. The optional dependency names
`tikv-jemallocator` and `mimalloc` are internal to those selections. The
binary entry points install a global allocator under the named `allocator-*`
features; a dependency-name feature would not select it.

## Rationale and evidence

Before D0001, each manifest used the bare optional dependency name in its
`allocator-*` feature list. Cargo consequently generated an additional
same-name feature for each optional dependency. That allowed a build to enable
the dependency without enabling the corresponding binary allocator selection.
All three manifests now use `dep:tikv-jemallocator` and `dep:mimalloc`. The
Cargo features reference documents that `dep:` suppresses the implicit
dependency-name feature.

## Alternatives and limits

Keeping the implicit names would expose build choices that do not match the
documented allocator selection. This correction changes Cargo feature names
available to callers; it does not change the selected allocator for any
documented `allocator-*` build. The three binary packages own their own
features, so check each package separately.

## Verification and re-evaluation

`cargo metadata --no-deps --format-version 1` lists only the three
`allocator-*` entries for allocator selection in each binary package after
the correction. Recheck the manifests, binary `#[global_allocator]` conditions,
and build examples if allocator dependencies or feature names change.

## Resolution references

- D0001: `tasks/todo/use-explicit-allocator-dependency-features.md` (original
  task, resolved by the three binary manifest changes).
