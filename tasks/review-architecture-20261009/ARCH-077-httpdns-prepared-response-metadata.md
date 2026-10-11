---
id: ARCH-077
status: confirmed
kind: architecture-debt
priority: P3
partition: PART-224
analyst: /root/analyze_conversion
baseline: 22f0fb8b6f4353f689ca4ddc172a4dfd98a2d2929dd1709240cbe4a1b3796471
---

# ARCH-077 — Prepare immutable HTTP DNS response metadata with its configuration generation

## Current ownership and evidence

Common already owns HTTP DNS query generations in `PreparedHttpDns`: `HttpDnsCandidate::prepare` builds the canonical client and copies only query policy, while `dns::resolve` borrows that immutable generation (`edgion-common/src/dns/mod.rs:118-138,270-272`). Gateway LinkSys publication and Controller per-Service state prepare or reuse that same core. Response selection and path conversion nevertheless remain query work.

For every successful fetch, `resolve_http_dns_with` calls `effective_response_format` and then `parse_body` (`edgion-common/src/dns/httpdns.rs:191-194`). Generic configuration clones `HttpDnsResponseFormat`, including configured `ip_path`, `delimiter` and the currently unused `ttl_path`; presets construct new owned fields (`httpdns.rs:68-72`; `edgion-resources/src/resources/link_sys/httpdns.rs:129-144`). Aliyun creates `ips` and `ttl` strings; Tencent creates its semicolon delimiter. For JSON, `parse_body` creates another owned JSON pointer from the same immutable dotted path on every parse (`httpdns.rs:75-89,107-111`). These fields do not depend on the queried domain or response document.

Gateway ExternalName resolution reaches this work when its address cache misses or is disabled; IP literals and cache hits bypass it (`edgion-gateway/src/backends/mod.rs:378-419`). Controller background resolution also queries the prepared core (`edgion-controller/src/services/dns_resolver/service.rs:329-343`). This is not an assertion that every proxied request performs HTTP DNS, nor a measured throughput or RSS regression.

## Root cause and bounded improvement

Configuration-independent response interpretation metadata is reconstructed below an existing immutable preparation boundary. Extend the private prepared query owner to select the effective preset/custom response mode once and encode its JSON pointer once. Production resolution should borrow this prepared metadata; each exchange still owns its response bytes, JSON document and bounded result list. Store only the fields needed for interpretation, rather than copying unused TTL metadata per query. No global memo, new resolver, general parsing registry, or new serialized cache is needed.

Preparation must be infallible metadata derivation, not new admission. In particular, JSON arrays do not use the delimiter at all, whereas JSON-string and delimited branches reject an explicitly empty delimiter only when splitting (`httpdns.rs:112-123`). Rejecting all empty delimiters during generation preparation would change accepted array responses and is outside this optimization. Preserve JSON syntax errors before missing-path/type/delimiter errors and retain the authored dotted path for existing diagnostics. Shared pure parsing logic can accept prepared borrowed metadata; injectable test wiring must not justify rebuilding immutable fields on the production query path.

The concrete change scenario is repeated queries against a generation with `ipPath: data.ips` or a vendor preset. Today each successful exchange selects/clones owned response fields and reconstructs the same pointer. After preparation, only document-dependent parsing repeats. Borrowing the generic response alone is a smaller alternative, but leaves vendor preset allocation and JSON pointer construction in production resolution. The existing generation owner provides a bounded place to remove all of this one cause without altering transport or cache policy.

## Required invariants and verification

Keep explicit `urlTemplate` precedence for the URL and preset precedence for the response format; these are intentionally separate. Preserve default `ips`, dotted-path escaping of `~` and `/`, empty components and array indexing, JSON-array versus string whitespace handling, ignored non-string/invalid IP candidates, bracketed IPv6, stable deduplication and the 256-address cap. Preserve configured fallback, primary-error precedence when fallback fails, and latest authored fallback even when client preparation fails. Do not implement TTL extraction or alter address-cache expiry.

Keep the authored configuration as the input to existing redacted address identity and private runtime identity. Derived response metadata must remain private runtime state; it must not change serialization, CRD fields, source authority, inherited TLS proofs, Secret/global replacement, ready/failed reuse, or the completion rights of borrowed generations. The canonical reusable HTTP client, System bootstrap, egress checks, disabled proxies, denied redirects, deadlines, cancellation, pool bounds and at-most-64-KiB streaming response cap remain unchanged. This separate exchange grants no main-response aggregation or response BodyStorage.

Existing `edgion-common/tests/unit/dns/httpdns_test.rs` covers nested paths, pointer metacharacters and array indices, array/string whitespace differences, delimiters, deduplication, the address cap, preset precedence with an explicit account URL, and fallback. Future verification should compare prepared parsing across those cases, add the empty-delimiter JSON-array distinction and failure-order checks, and confirm that repeated production queries do not clone response strings or rebuild the pointer. A generation replacement should update response interpretation while retaining the original identity/publication contracts. Existing transport/generation tests cover client reuse, failed fallback and cancellation; their historical recorded passes are not fresh execution in this review.

## Original decisions and scope limits

`skills/04-review/link-sys/regression-guards/httpdns-prepared-runtime.md` explicitly places reusable query policy and client/TLS auditing at immutable configuration boundaries, while preserving canonical cache identity, fallback and generation lifetimes. The HTTP DNS egress guard keeps endpoint dialing separate from returned-backend policy. Their accepted complexity is necessary transport/source ownership; neither requires reconstructing fixed response metadata on every successful query.

This proposal preserves the accepted necessary ownership of selected inline DNS server lists and makes no allocation-free DNS claim. Standard DNS resolver sharing would require separate native-cache/lifecycle analysis and is excluded. Original open resource-reference-contract work remains open; this is not permission to migrate other providers or Controller Secret acquisition. ARCH-076 owns transport error redaction separately. The demonstrated benefit is removal of specific avoidable owned string work; frequency, latency and process memory improvement were not benchmarked. Analysis was static and read-only.
