# Dead Code Candidates in src/core/common/ (refactored → multi-crate)

Total files scanned: 57  
Total public items catalogued: 253  
Total candidates identified: 25 (functions/methods with 0 external refs or only test-internal use)

---

## Functions with Zero External References (Never Called Outside common/)

These are `pub fn` / `pub async fn` items that are exported but have no callers outside
`src/core/common/`. Most are also not called from non-test code within `common/` itself.

| File:Line | Function | External Refs | Internal Use | Notes |
|-----------|----------|---------------|--------------|-------|
| `edgion-gateway/src/util/net.rs:14` | `parse_optional_listen_addr` | 0 | 0 | Defined but never called anywhere — REMOVED in refactor (not found 2026-06-13) |
| `edgion-gateway/src/util/net.rs:19` | `normalize_grpc_endpoint` | 0 | 0 | Defined but never called anywhere — REMOVED in refactor (not found 2026-06-13) |
| `edgion-gateway/src/util/net.rs:27` | `default_operator_addr` | 0 | 0 | Defined but never called anywhere — REMOVED in refactor (not found 2026-06-13) |
| `edgion-gateway/src/util/net.rs:74` | `is_localhost` | 0 | tests only | Wraps `classify_backend_addr`; doc says "kept for backward compatibility"; not called from non-test code |
| `edgion-gateway/src/util/mod.rs:74` | `check_need_version` | 0 | tests only | Only referenced in `mod tests` within same file — REMOVED in refactor (not found 2026-06-13) |
| `edgion-gateway/src/util/mod.rs:127` | `format_resource_info` | 0 | 0 | Dead — not called anywhere at all — REMOVED in refactor (not found 2026-06-13) |
| `edgion-resources/src/matcher/ip_cidr_helpers.rs:43` | `to_cidr_format` | 0 | called by `build_radix_matcher` only | Internal helper only; `build_radix_matcher` is the external-facing fn |
| `edgion-resources/src/matcher/ip_radix_tree/types.rs:134` | `u32_to_ipv4` | 0 | tests only | Only referenced in unit tests within same file |
| `edgion-resources/src/matcher/ip_radix_tree/types.rs:139` | `u128_to_ipv6` | 0 | tests only | Only referenced in unit tests within same file |
| `edgion-gateway/src/util/real_ip_extractor.rs:264` | `extract_ip_string` | 0 (re-exported via `util/mod.rs`) | tests only | Re-exported at `util` level but never imported elsewhere; only test callers |
| `edgion-gateway/src/plugins/http/openid_connect/jwt_common.rs:16` | `default_allowed_algs` | 0 | called by `resolve_algorithm_policy` only | Wrapper only called internally; `resolve_algorithm_policy` is the external-facing fn |
| `edgion-gateway/src/plugins/http/openid_connect/jwks_provider.rs:37` | `no_op_validator` | 0 | used within `jwks_provider.rs` and `jwt_middleware.rs` (both in gateway plugin/) | Internal construction helper; no external callers |
| `edgion-common/src/observe/metrics_api.rs:114` | `health_handler` | 0 | used in `create_metrics_router()` only (same file) | Only wired up internally |
| `edgion-controller/src/unified_auth/mod.rs:300` | `unified_auth_middleware` | 0 (external) | used in `common/api/compose.rs` and tests within same file | Wired through `compose_admin_routes`; not imported directly from outside common/ — REMOVED in refactor (not found 2026-06-13) |

---

## Methods with Zero External References

Public methods on types that are never called from outside `src/core/common/`:

| File:Line | Method | Type | External Refs | Notes |
|-----------|--------|------|---------------|-------|
| `edgion-resources/src/matcher/ip_radix_tree/matcher.rs:91` | `has_ipv4_rules` | `IpRadixMatcher` | 0 | Only in unit tests within same file |
| `edgion-resources/src/matcher/ip_radix_tree/matcher.rs:96` | `has_ipv6_rules` | `IpRadixMatcher` | 0 | Only in unit tests within same file |
| `edgion-resources/src/matcher/ip_radix_tree/matcher.rs:212` | `ipv4_rule_count` | `IpRadixMatcherBuilder` | 0 | Only in unit tests within same file |
| `edgion-resources/src/matcher/ip_radix_tree/matcher.rs:217` | `ipv6_rule_count` | `IpRadixMatcherBuilder` | 0 | Only in unit tests within same file |
| `edgion-resources/src/matcher/ip_radix_tree/matcher.rs:101` | `stats` → `MatcherStats` | `IpRadixMatcher` | 0 | `MatcherStats` struct also has 0 external refs |
| `edgion-resources/src/matcher/ip_radix_tree/frozen_v4.rs:142` | `stats` → `TreeStats` | `FrozenIpV4RadixTree` | 0 | Called by `IpRadixMatcher::stats()` internally |
| `edgion-resources/src/matcher/ip_radix_tree/frozen_v6.rs:142` | `stats` → `TreeStats` | `FrozenIpV6RadixTree` | 0 | Called by `IpRadixMatcher::stats()` internally |
| `edgion-resources/src/matcher/ip_radix_tree/frozen_v4.rs:137` | `node_count` | `FrozenIpV4RadixTree` | 0 | Used only by `IpRadixMatcher::stats()` |
| `edgion-resources/src/matcher/ip_radix_tree/frozen_v6.rs:137` | `node_count` | `FrozenIpV6RadixTree` | 0 | Used only by `IpRadixMatcher::stats()` |
| `edgion-resources/src/matcher/radix_tree/frozen.rs:548` | `match_exact` | `FrozenRadixTree` (alias `RadixTree`) | 0 | Only in unit tests; never called from production code |
| `edgion-resources/src/matcher/radix_tree/frozen.rs:342` | `match_all_prefixes` | `FrozenRadixTree` | 0 (external) | Called by `RadixHostMatchEngine::match_host`; both internal to common/ |
| `edgion-resources/src/matcher/radix_tree/frozen.rs:567` | `stats` → `TreeStats` | `FrozenRadixTree` | 0 | Never called from production code; no external callers |
| `edgion-resources/src/matcher/host_match/radix_match/radix_host_match.rs:51` | `export_hosts` | `RadixHostMatchEngine` | 0 | Defined but never called anywhere — REMOVED in refactor (not found 2026-06-13) |
| `edgion-resources/src/matcher/host_match/radix_match/radix_host_match.rs:45` | `tree` | `RadixHostMatchEngine` | 0 (external) | Only used in one internal unit test |
| `edgion-controller/src/local_auth/handlers.rs:87/193/206` | `login_handler`, `me_handler`, `logout_handler` | — | 0 (external) | Registered via `compose_admin_routes` within common/; not imported directly — REMOVED in refactor (not found 2026-06-13) |

---

## Structs/Enums with Zero External References

Items whose type name has 0 import/usage references outside `src/core/common/`:

| File:Line | Item | Type | External Refs | Notes |
|-----------|------|------|---------------|-------|
| `edgion-resources/src/matcher/ip_radix_tree/matcher.rs:113` | `MatcherStats` | struct | 0 | Return type of `IpRadixMatcher::stats()`, which itself has 0 external callers |
| `edgion-resources/src/matcher/ip_radix_tree/frozen_v4.rs:152` | `TreeStats` | struct | 0 | Return type of internal `stats()` methods; both definitions (v4 and v6) are dead |
| `edgion-resources/src/matcher/ip_radix_tree/frozen_v6.rs:152` | `TreeStats` | struct | 0 | (same as above) |
| `edgion-resources/src/matcher/radix_tree/frozen.rs:582` | `TreeStats` (third definition) | struct | 0 | Return type of `FrozenRadixTree::stats()`, which is never called |
| `edgion-gateway/src/plugins/http/openid_connect/jwks_provider.rs:57` | `LeanDiscoveryDocument` | struct | 0 (external) | Used within `jwks_provider.rs` (same plugin); not imported elsewhere |
| `edgion-resources/src/matcher/ip_radix_tree/types.rs:13` | `IpCidr` | enum | 0 (external) | Internal implementation detail for the IP tree builders; `IpCidr::parse()` is only used inside builder_v4/v6 |
| `edgion-resources/src/matcher/ip_radix_tree/types.rs:115,120,125` | `IpCidr::is_v4`, `is_v6`, `prefix_len` | methods | 0 | Methods on `IpCidr` that are never called anywhere (pattern matching used directly instead) |

---

## Potentially Dead Public API — Low-Confidence (May Be Intentional)

These items have 0 external callers but may be intentionally public for future use or
library-style API surface:

| Item | File | Reason it might be intentional |
|------|------|--------------------------------|
| `IpV4RadixBuilder`, `IpV6RadixBuilder` (pub use re-exports) | `edgion-resources/src/matcher/ip_radix_tree/mod.rs:111-112` | Re-exported as library surface; `IpRadixMatcherBuilder` wraps them internally |
| `FrozenIpV4RadixTree`, `FrozenIpV6RadixTree` (pub use re-exports) | `edgion-resources/src/matcher/ip_radix_tree/mod.rs:114-115` | Re-exported; only used internally inside `matcher.rs` |
| `IpCidr` (pub use re-export) | `edgion-resources/src/matcher/ip_radix_tree/mod.rs:117` | Re-exported; only used inside builder_v4/v6 |

---

## Summary of Strongest Dead Code Candidates

These are the highest-confidence items with no production callers anywhere:

1. **`format_resource_info`** (`edgion-gateway/src/util/mod.rs:127`) — Not called anywhere, period — REMOVED in refactor (not found 2026-06-13).
2. **`normalize_grpc_endpoint`** (`edgion-gateway/src/util/net.rs:19`) — Not called anywhere, period — REMOVED in refactor (not found 2026-06-13).
3. **`parse_optional_listen_addr`** (`edgion-gateway/src/util/net.rs:14`) — Not called anywhere, period — REMOVED in refactor (not found 2026-06-13).
4. **`default_operator_addr`** (`edgion-gateway/src/util/net.rs:27`) — Not called anywhere, period — REMOVED in refactor (not found 2026-06-13).
5. **`export_hosts`** (`edgion-resources/src/matcher/host_match/radix_match/radix_host_match.rs:51`) — Not called anywhere, period — REMOVED in refactor (not found 2026-06-13).
6. **`is_localhost`** (`edgion-gateway/src/util/net.rs:74`) — Only in tests; doc says "kept for backward compatibility" but no callers remain.
7. **`check_need_version`** (`edgion-gateway/src/util/mod.rs:74`) — Only in unit tests within same file — REMOVED in refactor (not found 2026-06-13).
8. **`u32_to_ipv4` / `u128_to_ipv6`** (`edgion-resources/src/matcher/ip_radix_tree/types.rs:134,139`) — Only in unit tests.
9. **`has_ipv4_rules` / `has_ipv6_rules`** (`edgion-resources/src/matcher/ip_radix_tree/matcher.rs:91,96`) — Only in unit tests.
10. **`ipv4_rule_count` / `ipv6_rule_count`** (`edgion-resources/src/matcher/ip_radix_tree/matcher.rs:212,217`) — Only in unit tests.
11. **`IpRadixMatcher::stats` + `MatcherStats` struct** (`edgion-resources/src/matcher/ip_radix_tree/matcher.rs`) — Never called from non-test, non-common code.
12. **`FrozenRadixTree::stats` + `TreeStats` struct** (`edgion-resources/src/matcher/radix_tree/frozen.rs`) — Never called anywhere.
13. **`FrozenRadixTree::match_exact`** (`edgion-resources/src/matcher/radix_tree/frozen.rs:548`) — Only called in unit tests within common/.
14. **`IpCidr::is_v4`, `is_v6`, `prefix_len` methods** (`edgion-resources/src/matcher/ip_radix_tree/types.rs:115,120,125`) — Never called; direct pattern matching used instead.
