# Dead Code Candidates in src/core/common/

Total files scanned: 57  
Total public items catalogued: 253  
Total candidates identified: 25 (functions/methods with 0 external refs or only test-internal use)

---

## Functions with Zero External References (Never Called Outside common/)

These are `pub fn` / `pub async fn` items that are exported but have no callers outside
`src/core/common/`. Most are also not called from non-test code within `common/` itself.

| File:Line | Function | External Refs | Internal Use | Notes |
|-----------|----------|---------------|--------------|-------|
| `utils/net.rs:14` | `parse_optional_listen_addr` | 0 | 0 | Defined but never called anywhere |
| `utils/net.rs:19` | `normalize_grpc_endpoint` | 0 | 0 | Defined but never called anywhere |
| `utils/net.rs:27` | `default_operator_addr` | 0 | 0 | Defined but never called anywhere |
| `utils/net.rs:85` | `is_localhost` | 0 | tests only | Wraps `classify_backend_addr`; doc says "kept for backward compatibility"; not called from non-test code |
| `utils/mod.rs:74` | `check_need_version` | 0 | tests only | Only referenced in `mod tests` within same file |
| `utils/mod.rs:127` | `format_resource_info` | 0 | 0 | Dead — not called anywhere at all |
| `matcher/ip_cidr_helpers.rs:38` | `to_cidr_format` | 0 | called by `build_radix_matcher` only | Internal helper only; `build_radix_matcher` is the external-facing fn |
| `matcher/ip_radix_tree/types.rs:134` | `u32_to_ipv4` | 0 | tests only | Only referenced in unit tests within same file |
| `matcher/ip_radix_tree/types.rs:139` | `u128_to_ipv6` | 0 | tests only | Only referenced in unit tests within same file |
| `utils/real_ip_extractor.rs:252` | `extract_ip_string` | 0 (re-exported via `utils/mod.rs`) | tests only | Re-exported at `utils` level but never imported elsewhere; only test callers |
| `jwt_common.rs:16` | `default_allowed_algs` | 0 | called by `resolve_algorithm_policy` only | Wrapper only called internally; `resolve_algorithm_policy` is the external-facing fn |
| `auth/jwks_provider.rs:32` | `no_op_validator` | 0 | used within `jwks_provider.rs` and `jwt_middleware.rs` (both in common/) | Internal construction helper; no external callers |
| `observe/metrics_api.rs:114` | `health_handler` | 0 | used in `create_metrics_router()` only (same file) | Only wired up internally |
| `unified_auth/mod.rs:300` | `unified_auth_middleware` | 0 (external) | used in `common/api/compose.rs` and tests within same file | Wired through `compose_admin_routes`; not imported directly from outside common/ |

---

## Methods with Zero External References

Public methods on types that are never called from outside `src/core/common/`:

| File:Line | Method | Type | External Refs | Notes |
|-----------|--------|------|---------------|-------|
| `matcher/ip_radix_tree/matcher.rs:85` | `has_ipv4_rules` | `IpRadixMatcher` | 0 | Only in unit tests within same file |
| `matcher/ip_radix_tree/matcher.rs:90` | `has_ipv6_rules` | `IpRadixMatcher` | 0 | Only in unit tests within same file |
| `matcher/ip_radix_tree/matcher.rs:168` | `ipv4_rule_count` | `IpRadixMatcherBuilder` | 0 | Only in unit tests within same file |
| `matcher/ip_radix_tree/matcher.rs:173` | `ipv6_rule_count` | `IpRadixMatcherBuilder` | 0 | Only in unit tests within same file |
| `matcher/ip_radix_tree/matcher.rs:95` | `stats` → `MatcherStats` | `IpRadixMatcher` | 0 | `MatcherStats` struct also has 0 external refs |
| `matcher/ip_radix_tree/frozen_v4.rs:159` | `stats` → `TreeStats` | `FrozenIpV4RadixTree` | 0 | Called by `IpRadixMatcher::stats()` internally |
| `matcher/ip_radix_tree/frozen_v6.rs:159` | `stats` → `TreeStats` | `FrozenIpV6RadixTree` | 0 | Called by `IpRadixMatcher::stats()` internally |
| `matcher/ip_radix_tree/frozen_v4.rs:154` | `node_count` | `FrozenIpV4RadixTree` | 0 | Used only by `IpRadixMatcher::stats()` |
| `matcher/ip_radix_tree/frozen_v6.rs:154` | `node_count` | `FrozenIpV6RadixTree` | 0 | Used only by `IpRadixMatcher::stats()` |
| `matcher/radix_tree/frozen.rs:548` | `match_exact` | `FrozenRadixTree` (alias `RadixTree`) | 0 | Only in unit tests; never called from production code |
| `matcher/radix_tree/frozen.rs:342` | `match_all_prefixes` | `FrozenRadixTree` | 0 (external) | Called by `RadixHostMatchEngine::match_host`; both internal to common/ |
| `matcher/radix_tree/frozen.rs:567` | `stats` → `TreeStats` | `FrozenRadixTree` | 0 | Never called from production code; no external callers |
| `matcher/host_match/radix_match/radix_host_match.rs:51` | `export_hosts` | `RadixHostMatchEngine` | 0 | Defined but never called anywhere |
| `matcher/host_match/radix_match/radix_host_match.rs:45` | `tree` | `RadixHostMatchEngine` | 0 (external) | Only used in one internal unit test |
| `local_auth/handlers.rs:87/193/206` | `login_handler`, `me_handler`, `logout_handler` | — | 0 (external) | Registered via `compose_admin_routes` within common/; not imported directly |

---

## Structs/Enums with Zero External References

Items whose type name has 0 import/usage references outside `src/core/common/`:

| File:Line | Item | Type | External Refs | Notes |
|-----------|------|------|---------------|-------|
| `matcher/ip_radix_tree/matcher.rs:107` | `MatcherStats` | struct | 0 | Return type of `IpRadixMatcher::stats()`, which itself has 0 external callers |
| `matcher/ip_radix_tree/frozen_v4.rs:169` | `TreeStats` | struct | 0 | Return type of internal `stats()` methods; both definitions (v4 and v6) are dead |
| `matcher/ip_radix_tree/frozen_v6.rs:169` | `TreeStats` | struct | 0 | (same as above) |
| `matcher/radix_tree/frozen.rs:582` | `TreeStats` (third definition) | struct | 0 | Return type of `FrozenRadixTree::stats()`, which is never called |
| `auth/jwks_provider.rs:51` | `LeanDiscoveryDocument` | struct | 0 (external) | Used within `jwt_middleware.rs` (same common/ package); not imported elsewhere |
| `matcher/ip_radix_tree/types.rs:13` | `IpCidr` | enum | 0 (external) | Internal implementation detail for the IP tree builders; `IpCidr::parse()` is only used inside builder_v4/v6 |
| `matcher/ip_radix_tree/types.rs:115,120,125` | `IpCidr::is_v4`, `is_v6`, `prefix_len` | methods | 0 | Methods on `IpCidr` that are never called anywhere (pattern matching used directly instead) |

---

## Potentially Dead Public API — Low-Confidence (May Be Intentional)

These items have 0 external callers but may be intentionally public for future use or
library-style API surface:

| Item | File | Reason it might be intentional |
|------|------|--------------------------------|
| `IpV4RadixBuilder`, `IpV6RadixBuilder` (pub use re-exports) | `ip_radix_tree/mod.rs:106-107` | Re-exported as library surface; `IpRadixMatcherBuilder` wraps them internally |
| `FrozenIpV4RadixTree`, `FrozenIpV6RadixTree` (pub use re-exports) | `ip_radix_tree/mod.rs:109-110` | Re-exported; only used internally inside `matcher.rs` |
| `IpCidr` (pub use re-export) | `ip_radix_tree/mod.rs:112` | Re-exported; only used inside builder_v4/v6 |

---

## Summary of Strongest Dead Code Candidates

These are the highest-confidence items with no production callers anywhere:

1. **`format_resource_info`** (`utils/mod.rs:127`) — Not called anywhere, period.
2. **`normalize_grpc_endpoint`** (`utils/net.rs:19`) — Not called anywhere, period.
3. **`parse_optional_listen_addr`** (`utils/net.rs:14`) — Not called anywhere, period.
4. **`default_operator_addr`** (`utils/net.rs:27`) — Not called anywhere, period.
5. **`export_hosts`** (`matcher/host_match/radix_match/radix_host_match.rs:51`) — Not called anywhere, period.
6. **`is_localhost`** (`utils/net.rs:85`) — Only in tests; doc says "kept for backward compatibility" but no callers remain.
7. **`check_need_version`** (`utils/mod.rs:74`) — Only in unit tests within same file.
8. **`u32_to_ipv4` / `u128_to_ipv6`** (`ip_radix_tree/types.rs:134,139`) — Only in unit tests.
9. **`has_ipv4_rules` / `has_ipv6_rules`** (`ip_radix_tree/matcher.rs:85,90`) — Only in unit tests.
10. **`ipv4_rule_count` / `ipv6_rule_count`** (`ip_radix_tree/matcher.rs:168,173`) — Only in unit tests.
11. **`IpRadixMatcher::stats` + `MatcherStats` struct** — Never called from non-test, non-common code.
12. **`FrozenRadixTree::stats` + `TreeStats` struct** (radix_tree) — Never called anywhere.
13. **`FrozenRadixTree::match_exact`** — Only called in unit tests within common/.
14. **`IpCidr::is_v4`, `is_v6`, `prefix_len` methods** — Never called; direct pattern matching used instead.
