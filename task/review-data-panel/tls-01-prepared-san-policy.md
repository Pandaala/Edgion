# Prepared SAN allowlist policy

Status: deferred design decision. Source baseline: `2c9cd2807`.

## Evidence

The Gateway leaf verification chain is
`mtls_verify_callback::validate_certificate_whitelist` →
`tls::validation::mtls::validate_san_whitelist` →
`edgion_common::allowed_san::client_sans_match_allowed`.
The callback skips absent/empty SAN constraints and extracts typed peer SANs once
for a nonempty list. CN matching is an exact borrowed string comparison; no
additional CN matcher is proposed here.

Common checks every allowlist entry with `allowed_san_schema_error` before it
tries any candidate. This repeats trimming, OID grammar checks, DNS hostname
regex matching or IP parsing, depending on entry type. Invalid entries reject
the whole list, including when another entry would match. Uncompiled regex
entries also reject the whole list. Entry errors construct temporary strings.

Candidate matching trims each entry value again. IP Exact reparses the allowed
address and presented text for each comparison. Case-insensitive Contains builds
lowercase copies of both strings for each same-type candidate comparison;
case-insensitive Exact/Prefix/Suffix already use borrowed ASCII comparisons.
DNS Exact uses the canonical bidirectional RFC 6125 matcher.

Gateway publication already compiles regex entries in
`tls/store/tls_store.rs::compile_allowed_sans_for_runtime`, then validates and
publishes the final resource generation. Resources' `AllowedSan::compile` only
prepares regexes; its skipped `compiled_regex` field is reconstructed after
serde. The Common validator is shared by Controller and Gateway, but the current
runtime matcher call sites are Gateway-owned. Do not infer that admission alone
allows removing runtime invalid/uncompiled rejection.

## Decision alternatives

- Prepare an immutable validated policy at publication: retain whole-list
  rejection state, normalized immutable operands and parsed IPs with the exact
  resource generation. Choose a canonical owner usable without creating a
  Controller-to-Gateway dependency. Integrate its lifetime with the existing
  compiled TLS generation; do not add a per-handshake or global policy cache.
- Make a narrower Contains-only optimization after comparing algorithms.
  Byte-window ASCII comparisons avoid lowercase allocations but can perform
  O(haystack length × needle length) work on repetitive near-misses. Allocation
  reduction alone does not establish a data-plane improvement over the current
  standard-library substring search. A prepared searcher adds policy memory and
  an algorithm/ownership choice that also needs evidence.

The user should choose whether to pursue prepared-policy ownership or the
narrower measured search change. Neither is implemented by this task. Keep the
existing runtime validation until a reviewed replacement preserves its contract.

## Acceptance

Preserve whole-list invalid/uncompiled rejection even with an earlier matching
entry; absent/empty constraints; typed SAN and OtherName OID isolation; trimmed
operator operands; DNS wildcard matching in both directions; IP canonicalization
without newly equating IPv4 and IPv4-mapped IPv6; and literal ASCII case behavior
for non-regex strings. Regex Unicode folding, inline flag overrides and compiler
bounds remain unchanged. Preserve SAN-before-CN rejection and sensitive failure
diagnostics.

Exercise serde reconstruction, full/partial publication, invalid-update
last-known-good retention, retained old generations and repeated policy changes.
Derived fields must remain outside schema/transport, and resource cloning must
not lose final-generation attachment prewarming. Peer certificate identity is
connection-specific and must not be cached in configuration.

Compare allocation counts, retained policy memory and CPU for short/long input,
first/late/no match, repetitive near-misses, ASCII and multibyte text. Report
which validation/parsing/projection costs remain. Existing Common and Resources
tests establish selected semantics, not workload performance or full Contains
differential coverage. A future change needs its own exact cross-crate allow-list,
IMPL plan/final review, targeted tests and repository gates.

Relevant original knowledge:
`skills/04-review/tls/regression-guards/allowed-san-regex-case-mode.md`,
`skills/04-review/tls/regression-guards/edgion-tls-incremental-atomic-publication.md`,
and `skills/04-review/tls/intentional-design/last-known-good-follows-resource-key.md`.

## Investigation verification

Fresh selected tests passed: 14 Common allowlist tests and 11 Resources allowlist
tests. Formatting, workspace all-target check and Clippy passed; Clippy retains
warnings outside this documentation-only change. No production code was changed,
no new search algorithm was tested, and no live handshake or benchmark was run.
