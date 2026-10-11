import sys
sys.path.insert(0, 'tasks/review-architecture-20261009/.tools')
import record

body = '''
# Move the retiring LRU node key instead of cloning it

`edgion-algo/src/cache/lru.rs::LocalLruCacheInner::remove_index` clones the occupied node's `K` solely to remove its corresponding map entry. The function then releases the node, whose reset drops the original key. The removal owns the node's entire retirement under the shard mutex; the original node key is not needed after that operation. The duplicate owned key is discarded immediately rather than retained by another generation or consumer.

This is the canonical release funnel for explicit removal, lazy expiration from get, full-cache expiration pruning and LRU displacement. Those callers all converge on the same private function, so a local ownership correction covers their behavior without changing ShardedLruCache, the public borrowed-key API or any cache policy.

The request-path witness is concrete. Gateway `backends/mod.rs::resolve_external_ips_with` calls the canonical DnsCache get/insert during cached ExternalName self-resolution. DnsCache stores `DnsCacheKey`, whose three fields—service identity, external hostname and resolver fingerprint—are owned Strings. Expired lookup and capacity-pressure insertion reach remove_index and clone that whole key while holding the shard mutex. OIDC request_filter_impl also resolves opaque session references through token_exchange's String-keyed access-token cache; recent-refresh results use another String-keyed canonical cache. These keys are not full bearer tokens. The proposed benefit is removal of one unnecessary key clone per retired entry, including associated String allocations for these actual consumers; no measured latency, RSS, incident or larger hot-path claim is made.

Take the retiring node's existing key into a local owned K before its reset, then use a reference to that key for map removal. Keep the occupied-node guard and missing-key invariant check. Preserve the existing release obligations: remove the exact expiration record before clearing its deadline, detach the same list node, remove the map entry, decrement occupancy once, return the original value and put precisely that slot on the free list. All of these remain in the current mutex critical section. No new cache, closure API, key interning, Arc migration, extra lock or ownership framework is required. Borrowing the key directly for map removal before later node mutations is an alternative, provided Rust's field borrowing and operation order remain clear; moving the already-retired key makes the lifetime explicit.

The key clone during insert has a different ownership requirement: the map and node both retain a key after insertion, so this proposal leaves it alone. Cache hits likewise continue returning the caller-selected cloned V; consumers may choose Arc values as the current contract states. Hashing, shard selection, quotient/remainder capacities, TTL arithmetic, zero-TTL removal, strict replacement, lazy expiration, ordered index, defensive progress guard, approximate stats and cold get_keys sweeps remain unchanged.

The original bounded-expiration-index decision explicitly requires the exact release funnel and its record/list/free-slot invariants, not a key clone. This proposal does not reopen its accepted BTreeSet tradeoff, pruning schedule or counters. The original cache-selection rule continues to require this canonical strict/explicit-invalidation cache; TinyUFO's separately accepted positive-introspection use is unaffected.

All local and sharded LRU test bodies were read, including repeated full-capacity eviction, expired-before-live pruning, rewritten deadlines, explicit removal, clear/free-slot reuse, borrowed String keys and concurrent shard/index bounds. DNS tests additionally cover capacity churn and expired-entry reclamation before live eviction, and OIDC tests cover TTL-backed recent-result freshness and explicit access-token removal. Future verification should retain those behaviors and confirm by source inspection that remove_index retires the original node key without K::clone. No allocator benchmark or production visibility expansion is necessary. Exact source/review searches found no existing finding for this release-key ownership cause. Static read-only review only; no code, tests, builds, network or Git operations were executed or changed.
'''
record.candidate('ARCH-186', 'lru-retiring-key-ownership', 'PART-150', '/root/analyze_outbound', 'architecture-debt', 'P3', body)
base = {
    'AGENTS.md': 'Full personal original retained, fresh unchanged hashes, root-only AGENTS.',
    'skills/SKILL.md': 'Full personal original retained/current topic lookup.',
    'skills/00-design-rules.md': 'Full personal original graded rules retained/relevant gates refreshed.',
    'skills/04-review/SKILL.md': 'Full original retained, current symbol routing/dedup.',
    'skills/04-review/knowledge-routing.md': 'Full original retained.',
    '/Users/caohao/.codex/skills/edgion-architecture-review/SKILL.md': 'Full original active workflow.',
    'edgion-algo/Cargo.toml': 'Full external-only manifest/benchmarks.',
    'edgion-algo/src/lib.rs': 'Full support exports; zero primary credit.',
    'skills/12-algorithms/SKILL.md': 'Full original catalog/selection/replacement contracts.',
}
supports = dict(base)
supports.update({
    'edgion-algo/tests/unit/cache/lru_test.rs': 'Full607 lines/every actual body.',
    'edgion-algo/tests/unit/cache/sharded_lru_test.rs': 'Full143 lines/every actual body.',
    'edgion-resources/src/resources/edgion_plugins/plugin_configs/lru_cache_config.rs': 'Full shared capacity/shards/cold validation.',
    'skills/04-review/cpu-memory/regression-guards/lru-bounded-expiration-index.md': 'Full original exact release/index invariants, accepted tradeoffs; history not rerun.',
    'skills/04-review/cpu-memory/regression-guards/oidc-access-token-cache-ttl-only.md': 'Full original TTL lifetime.',
    'skills/01-architecture/00-common/05-unification-points.md': 'Retained full registry, current1-72 canonical cache.',
    'edgion-gateway/src/backends/discovery/dns_cache.rs': '1-280 actual modes/3String keys/bounded canonical get/insert/invalidation.',
    'edgion-gateway/src/backends/mod.rs': '172-269,270-350,356-422 actual request self-resolution through cache.',
    'edgion-gateway/tests/unit/backends/discovery/dns_cache_test.rs': '105-155,245-316 complete selected actual hit/identity/churn/prune tests.',
    'edgion-gateway/src/plugins/http/openid_connect/token_exchange.rs': '1-82,348-395 actual boundedTTL access-token/recent-result cache.',
    'edgion-gateway/src/plugins/http/openid_connect/request_filter_impl.rs': '45-105 actual session-cache/refresh entry.',
    'edgion-gateway/src/plugins/http/openid_connect/plugin.rs': '277-290,425-447 actual cache fields/construction.',
    'edgion-gateway/tests/unit/plugins/http/openid_connect/plugin_test.rs': '2155-2188,2374-2404 complete selected TTL/removal tests.',
    'edgion-gateway/src/plugins/http/wasm/oci/cache.rs': '145-205 canonical cache/monotonic TTL cold gate.',
})
record.partition('PART-150', '/root/analyze_outbound', {
    'edgion-algo/src/cache/lru.rs': 'Full554 lines; ARCH-186 retirement key clone; all list/map/free-slot/expiration/strictTTL/stats seams traced.',
    'edgion-algo/src/cache/mod.rs': 'Full5 lines/export consumers; no issue.',
    'edgion-algo/src/cache/sharded_lru.rs': 'Full152 lines; exact total capacity/hash/borrowed operations and deliberately torn aggregates preserved; no issue.',
}, supports, ['ARCH-186'], 'Static full primary/actual tests; no execution/mutations/network/Git. No measured performance claim. Overflow lead not promoted: actual OIDC/DNS/OCI consumers bound or check deadlines. Support zero primary credit.')

body = '''
# Compile mapped IPv6 CIDRs into the address family used by the unified matcher

IpRadixMatcherBuilder::insert in edgion-algo/src/matcher/radix_ip/matcher.rs parses a CIDR, interns its group and chooses the IPv4 or IPv6 builder from the parsed family. IpCidr::parse preserves the IPv6 family for IPv4-mapped addresses, including mapped-only subnets such as ::ffff:192.0.2.0/120. The unified matcher therefore accepts and freezes that rule in its IPv6 tree.

IpRadixMatcher::match_label_id deliberately queries only the IPv4 tree for both native IPv4 and IPv4-mapped IPv6 inputs. A mapped input is converted using to_ipv4_mapped before lookup; it never queries the IPv6 tree. Every address covered by the example mapped-only subnet takes that branch. The accepted rule is consequently unreachable through the public unified matcher. With that rule alone, both 192.0.2.7 and ::ffff:192.0.2.7 return no match. This is a source-level witness, not an executed traffic observation.

The mismatch reaches actual policy consumers. Resources ip_cidr_helpers::validate_ip_or_cidr accepts the IPv6 prefix length, and to_cidr_format passes CIDR text through unchanged. collect_entry_errors reports no error for a single valid named group containing this rule; build_grouped_matcher_partial inserts it successfully. IpRestrictionConfig::validate_and_init and the production IpRestriction::create_bound constructor therefore complete without init_failed. For a deny-only configuration containing this mapped subnet, with defaultAction Allow and no allow references, the actual evaluate_ip_access_multi path finds no deny match and returns Default(Allow). The HTTP plugin continues with its existing allow:default outcome. An allow-only configuration instead over-blocks the corresponding address through its unchanged whitelist-miss behavior. This finding concerns an accepted configured CIDR, not a missing constructor initializer, a malformed entry deliberately skipped by partial-good compilation, or an SSRF classifier bypass.

Keep mapped query normalization and place corresponding mapped-rule normalization at the existing unified builder boundary. A parsed IPv6 CIDR wholly contained in ::ffff:0:0/96 has a prefix length from 96 through 128 and a mapped normalized address. Compile it as the embedded IPv4 address with prefix_len minus 96, using the existing IPv4 binary radix core and group-label interning. A small private parsed-insertion seam in the family adapter can avoid manufacturing and reparsing another CIDR string; it requires no public generic framework, runtime policy state or request allocation.

Preserve raw IpCidr parsing and low-level IpV6RadixBuilder/FrozenIpV6RadixTree semantics. Those APIs describe actual IPv6 bit prefixes and should not silently acquire the unified matcher's family policy. Do not translate broader IPv6 prefixes below /96, IPv4-compatible addresses such as ::192.0.2.7, native IPv6 ranges, or ::1. In particular, an IPv6 /0 remains an IPv6 rule and does not become an IPv4 wildcard. This is a cold normalization correction for mapped-only CIDRs, not a new cross-family matching policy.

The structural benefit is one coherent family decision for insertion and lookup in the canonical unified owner, inherited by existing Controller, Resources and Gateway consumers. Adding a second runtime IPv6 fallback would alter documented family separation, add request work, and create conflicting cross-tree prefix and group precedence. Per-consumer rewrites would duplicate the canonical rule and could leave Admin allowlists, trusted-proxy rules, condition matchers or referenced IpLists inconsistent.

Preserve host-bit normalization, label zero reservation and MAX_LABELS, malformed-input and empty-group error ordering, same-label idempotence, different-label DuplicateCidr behavior, empty matcher versus empty builder behavior, longest-prefix matching, bounded traversal, frozen layout and immutable snapshots. Mapped and native aliases of one normalized IPv4 prefix must reach the same existing terminal ownership rules: same-group aliases coalesce, while different-group aliases retain the strict builder's duplicate error and the partial-good caller's existing first-kept behavior. Verify actual strict and partial consumers rather than introducing a new grouped validation or reporting policy. Existing advisory diagnostics, deny-first evaluation, configured whitelist intent, reference generation ownership and group attribution remain with their current owners.

The original ip-radix-matching algorithm contract explicitly establishes IPv4-only lookup for mapped inputs, separate family traversal and the existing frozen representation. It does not establish that mapped-only CIDRs are rejected or translated on insertion. The current parser, grouped validators and builder establish the opposite: they accept these rules unchanged. Its statement that mapped normalization is tested currently refers to mapped query inputs against native IPv4 rules. Actual matcher_test.rs contains two such query tests, but no mapped-CIDR insertion witness. The complete eight radix-IP test bodies were read, along with actual HTTP constructor/evaluator tests. They preserve family rejection, prefix limits, DFS indices, layout, /0, host routes, longest-prefix selection, duplicate labels and mapped-query behavior; none exercises this accepted unreachable rule.

Future verification should test mapped /96, /120 and /128 insertions against both native and mapped queries, with in-range and out-of-range controls; host-bit variants; native/mapped alias insertion in both orders and with same/different groups; and unchanged native IPv6, broader IPv6 prefix, IPv4-compatible and IPv6 default-route behavior. Exercise the actual grouped helper and production HTTP constructor/evaluator wiring for the deny-only default-Allow witness and the whitelist case, using the existing fixture owner and log assertions without extra production allocations for mocks. Retain the existing strict versus partial-good contracts and canonical access-log ownership. No new logging, metrics, schema fields, dependencies, network checks or alternate matcher are needed.

Relevant algorithm, unification, grouped-list, constructor initialization, deny/allow evaluation and mapped-address review originals were personally read. Existing review artifacts, pending tasks and knowledge were searched for mapped-CIDR insertion and insertion/query family mismatch; no matching finding was located. ARCH-028 concerns borrowed matcher unions, while the mapped SSRF review concerns address classification; neither owns this cause. The analysis is static and makes no deployed exploit, incident, benchmark or memory-measurement claim.
'''
record.candidate('ARCH-181', 'mapped-cidr-family-admission', 'PART-153', '/root/verify_ip_union', 'behavior-defect', 'P2', body)
supports = dict(base)
supports.update({
    'edgion-algo/src/matcher/mod.rs': 'Full supporting export seam.',
    'skills/12-algorithms/ip-radix-matching.md': 'Full original dispatch/layout/complexity/duplicates/bounds/replacement.',
    'skills/01-architecture/00-common/05-unification-points.md': '302-325 canonical IP/realIP.',
    'skills/01-architecture/00-common/04-grouped-list-pattern.md': 'Full original groups/strict-partial/observability; inventory tail refreshed.',
    'skills/04-review/ip-restriction/regression-guards/deny-allow-default-algorithm-extracted-to-evaluate-ip-access.md': 'Full original evaluator contract.',
    'skills/04-review/ip-restriction/regression-guards/http-create-must-call-validate-and-init.md': 'Original constructor/partial-good/fail-closed plus current create_bound.',
    'skills/04-review/ip-restriction/mandatory-design/group-name-in-deny-log.md': 'Full attribution contract.',
    'skills/04-review/ssrf/regression-guards/ipv4-mapped-ipv6.md': 'Original distinct mapped-classification decision.',
    'edgion-resources/src/matcher/ip_cidr_helpers.rs': 'Actual validation/group errors/strict-partial compilation/canonical evaluator.',
    'edgion-resources/src/resources/edgion_plugins/plugin_configs/ip_restriction.rs': '120-245 default/init/allow-mode/checks.',
    'edgion-gateway/src/plugins/http/ip_restriction/plugin.rs': '70-130 production create_bound,145-215 snapshots/evaluation/outcomes.',
    'edgion-gateway/tests/unit/plugins/http/ip_restriction/plugin_test.rs': 'Actual fixtures/full selected init-failure/partial/default/direct/whitelist tests; mapped absence search.',
    'edgion-controller/src/api/ip_allowlist.rs': 'Full allowlist builder/middleware.',
    'edgion-gateway/src/util/real_ip_extractor.rs': '75-125 actual trusted groups; rest searched only.',
    'edgion-resources/src/resources/proxy_protocol.rs': '300-345 actual raw canonicalize_cidr compatibility boundary.',
})
for name in ['binary','builder_v4','builder_v6','error','frozen_v4','frozen_v6','matcher','types']:
    supports[f'edgion-algo/tests/unit/matcher/radix_ip/{name}_test.rs'] = 'Full actual test bodies; all8 total554 lines.'
s = record.state()
dispositions = {p: 'Entire implementation read; canonical private binary core/adapters/frozen layout, bounded traversal and actual tests/consumers; no additional actionable finding.' for p in s['partitions']['PART-153']['paths']}
dispositions['edgion-algo/src/matcher/radix_ip/matcher.rs'] = 'Entire implementation read; ARCH-181 accepted mapped-only CIDR insertion/query family mismatch.'
record.partition('PART-153', '/root/verify_ip_union', dispositions, supports, ['ARCH-181'], 'Static full9 sources/eight actual tests/current contracts; no execution/build/test/network/Git/write or incident/performance claim. Supporting reading zero primary coverage.')
s = record.state()
s['support_followups'].append({'partition':'PART-150','topic':'Canonical LRU consumer inventory prose','evidence':'Bounded-expiration-index original says OIDC sole production consumer; current DNS and Wasm OCI canonical consumers also exist. Ownership/index choice remains accepted, prose-only inventory follow-up.'})
record.save(s)
for i in ['ARCH-181','ARCH-186']: print(i, record.state()['candidates'][i])
