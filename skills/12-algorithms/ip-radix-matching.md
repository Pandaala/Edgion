---
name: ip-radix-matching
description: Shared IPv4/IPv6 binary radix construction, frozen layout, and longest-prefix lookup contract.
status: shipped
---

# Binary radix matching — IP/CIDR policy

## What we run today

`edgion-algo/src/matcher/radix_ip/` owns the canonical IP/CIDR matcher used by
Controller, Resources, and Gateway policy consumers. `IpRadixMatcher` interns group
labels and keeps separate optional IPv4 and IPv6 trees. Native IPv4 and IPv4-mapped
IPv6 inputs query only the IPv4 tree; other IPv6 inputs query only the IPv6 tree.

Both address families use one private binary-radix core, monomorphized through a
two-implementation `BitKey` trait for `u32` and `u128`. Insertion consumes bits from
most significant to least significant and records a terminal label at the prefix
depth. Freeze emits nodes in DFS pre-order into a contiguous `Vec<FlatNode>`.
Lookup walks at most 32 or 128 edges and retains the last terminal encountered,
which implements longest-prefix matching.

## Contract

For R rules of address width W, construction is O(sum of prefix lengths), bounded
by O(RW). Freeze is O(N) time and memory for N build nodes. Lookup is O(W), with W
fixed at 32 or 128. It allocates no memory, performs no string conversion, takes no
lock, and uses no trait object or indirect runtime dispatch.

Each frozen node is 16 bytes with 8-byte alignment and stores two `u32` child
indices, terminal metadata, and explicit padding. The key type is not stored in a
node. Index zero is both the root index and the no-child sentinel; this is sound
because DFS pre-order assigns every real child an index of at least one. Separate
trees prevent cross-family traversal. The immutable trees are safe for concurrent
readers through their owning snapshots.

CIDR parsing normalizes host bits before insertion. A duplicate normalized CIDR
with the same label is idempotent; a different label is rejected. Empty builders,
invalid families, prefix bounds, label bounds, frozen layout, DFS indices, `/0`,
`/32`, `/128`, longest-prefix selection, mixed-family dispatch, and mapped IPv6
normalization are covered by unit tests.

## Why this algorithm

Policy matching needs deterministic longest-prefix results, bounded lookup work,
and a small allocation-free read path. A binary trie is simple enough to audit and
its maximum depth is fixed by the address family. The mutable boxed build form keeps
insertion straightforward, while freezing removes pointer chasing from lookups.

The shared core removes duplicated V4/V6 correctness logic without merging their
address spaces. Static monomorphization preserves family-specific widths and avoids
adding an address-family branch inside traversal.

## Prior art comparison

| Implementation | Decision and reason |
|---|---|
| [treebitmap](https://github.com/hroi/treebitmap) | Do not adopt without Edgion-specific benchmark evidence. Multi-bit strides and bitmaps can reduce traversal depth, but add representation complexity and do not by themselves justify replacing the bounded 16-byte binary nodes. |
| [Poptrie](https://github.com/nicolaskagami/poptrie) | Do not adopt without a demonstrated lookup or memory win on Edgion policy sizes. Its compressed popcount-based layout targets large forwarding tables and has a more complex build representation. |
| A linear `Vec<IpNet>` scan | Rejected because lookup cost grows with rule count and would duplicate the canonical matcher across policy consumers. |

## Known limitations

The uncompressed tree may allocate one build node per distinct prefix bit and can
use more memory than compressed or multi-bit tries on large routing tables. Freeze
temporarily retains both boxed and flat forms. Recursive build/freeze helpers have a
strict maximum depth of 128. No claim is made that this layout is optimal for
Internet-scale forwarding tables; Edgion uses it for bounded gateway policy rules.

## Replacement criteria

Consider a Patricia, Tree Bitmap, Poptrie, or another layout only after a
correctness-equivalent implementation measurably improves optimized lookup latency
or retained memory on representative Edgion IPv4, IPv6, mixed-prefix, sparse, and
shared-prefix rule sets. It must preserve separate family dispatch, duplicate and
label semantics, immutable snapshots, bounded construction failure, and
allocation-free lookup. Report the machine, profile, rule distributions, build
cost, lookup percentiles, and retained bytes.

## References

- `edgion-algo/src/matcher/radix_ip/`
- `edgion-resources/src/matcher/ip_cidr_helpers.rs`
- [Cross-cutting unification points](../01-architecture/00-common/05-unification-points.md)
- `tasks/pending/unify-ip-radix-v4-v6-core.md`
