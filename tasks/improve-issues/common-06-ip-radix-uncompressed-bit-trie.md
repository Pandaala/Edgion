# common-06: `radix_ip` is an uncompressed one-node-per-bit trie — ~100x memory amplification and up to 128 dependent loads per IPv6 lookup

- **Category**: best-practice
- **Severity**: P3 (downgraded from P2 by independent verification)
- **Scope**: local
- **Location**: `edgion-algo/src/matcher/radix_ip/binary.rs:74-97` (`insert_node`), `binary.rs:120-170` (`FlatNode`, `match_key`), `edgion-algo/src/matcher/radix_ip/matcher.rs:46-58`; consumers `edgion-resources/src/matcher/ip_cidr_helpers.rs:196,278` (IpGroup/IpList compilation)

## Summary
Despite the "radix" name, the IP matcher is a plain binary trie with no path compression: inserting a
prefix of length *L* walks/creates up to *L* nodes, one per bit, and lookup follows one node per bit.
For IPv4 lists this is tolerable; for IPv6 host entries (/128, /64) the frozen tree stores ~111 nodes ×
16 bytes per sparse entry and every lookup chases up to 128 dependent indices. IP lists are business
data (`EdgionConfigData` `IpList`, CLAUDE.md "Plugin IP list ownership"), with no size bound on
`IpGroup.cidrs`, so threat-intel / bot / cloud-range feeds are the expected input.

## Current behavior (evidence)
- `insert_node` (`binary.rs:74-97`) recurses bit by bit, `get_or_insert_with(Box::new(BuildNode::new()))`
  at each depth until `current_bit == prefix_len`.
- `FlatNode` (`binary.rs:120-127`): `left_index: u32, right_index: u32, prefix_len: u8, label_id: u16,
  _padding` under `repr(C, align(8))` → 16 bytes per bit-level node.
- `match_key` (`binary.rs:143-172`): loop of `self.nodes.get(node_idx)` + `bit_at` per bit, up to
  `K::WIDTH` (128 for IPv6) iterations, each a dependent random load in a large `Vec`.
- Build phase allocates one `Box<BuildNode>` per bit-node, then a recursive flatten — peak memory is
  the boxed tree plus the flat copy.
- `IpGroup.cidrs: Vec<String>` (`edgion-resources/src/resources/edgion_config_data/ip_group.rs:21`) has
  no count limit.

Rough cost for 50k random IPv6 /128 entries: shared top ~16 levels, then ~112 private nodes each →
~5.6M nodes ≈ 90 MB frozen (plus several times that transiently in `Box` nodes during build), versus
~1–2 MB for a path-compressed or LC-trie representation. The same list is compiled per Gateway process,
and a hot reload builds the new generation while the old one is still alive.

## Industry comparison
- Linux FIB uses an LC-trie (level- and path-compressed); DPDK uses DIR-24-8 for v4 and a multibit
  trie for v6.
- Envoy's `Network::LcTrie` (used for RBAC/IP-tagging CIDR sets) is level-compressed with a
  configurable fill factor and root branching factor.
- HAProxy ACL IP lists use ebtrees (path-compressed patricia); NGINX `geo` uses a radix tree for v4
  and a range-sorted array when `ranges` is set.

## Impact
Data-plane memory and reload latency scale with bits-per-entry instead of entries; IPv6 lookups on the
IP-ACL / RealIp / GeoIP-adjacent request path cost tens of cache misses where 3–6 would do. A large
IPv6 feed in one `IpList` can make every Gateway's RSS jump by hundreds of MB at reload.

## Suggested direction
Keep the public API and frozen/immutable design; change the representation:
1. **Patricia (path-compressed) nodes**: store `(skip_bits, prefix_bits)` per node so chains of
   single-child nodes collapse; node count becomes ≤ 2 × entries. Lookup does one masked compare per
   branching node. This is the smallest change and preserves first-match / longest-match modes.
2. Or a **multibit stride** layout (e.g. 16-8-8 for v4, 16-then-8-bit strides for v6, as DPDK/LC-trie)
   for fewer levels; more memory per node but O(L/stride) loads.
3. Add an operator-visible bound (entries or estimated bytes) at `IpList` admission so a single
   resource cannot blow up every Gateway, mirroring other bounded-compile rules (e.g. regex disjointness
   limits in `edgion-algo/src/matcher/regex_disjoint.rs`).

## Notes
Searched `skills/04-review/` (`radix_ip`, `binary trie`, `LcTrie`, `ip-restriction/`, `cpu-memory/`) and
`tasks/` — no intentional-design page accepting the per-bit layout, no tracked task. CLAUDE.md forbids a
size-based merge threshold and inline copies for IP lists; this proposal keeps the single centrally
compiled matcher and only changes its internal representation.

## Verification (independent)

- **Verdict**: DOWNGRADED
- **Severity**: P3 (was P2)
- `edgion-algo/src/matcher/radix_ip/binary.rs:74-97`: `insert_node` creates one `BuildNode` per bit until `prefix_len`.
- `binary.rs:120-131`: `FlatNode` is 16 bytes (`repr(C, align(8))`), one per bit-level node; `match_key` (`:143-175`) walks one node per bit up to `K::WIDTH`.
- No intentional-design page accepts the layout; no IpGroup size bound was found.
- Downgraded to P3: memory blow-up requires tens of thousands of sparse IPv6 host entries (most threat feeds are IPv4/aggregated), and lookup cost is still sub-microsecond; worth doing when large IpList feeds become a target.

## Re-verification (feature-10-28)

- **Verdict**: STILL-VALID
- **Severity**: P3
- `edgion-algo/src/matcher/radix_ip/` and `edgion-resources/src/matcher/` are unchanged; `insert_node` still creates one node per bit and `FlatNode` is still 16 bytes.
- `IpGroup.cidrs` still has no count bound.
