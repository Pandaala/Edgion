---
id: ARCH-058
status: candidate
kind: architecture-debt
priority: P3
partition: PART-200
analyst: /root/analyze_conversion
independent_verification: pending
---

# Retire the disconnected EdgionTls hostname matcher

## Current ownership and evidence

`edgion-resources/src/resources/edgion_tls.rs:446-553` exposes
`EdgionTls::matches_hostname` and implements its own private `wildcard_match`.
It lowercases the query and each configured host, scans the resource's host list,
and accepts consecutive leading wildcard labels such as `*.*.example.com`.
This is a separate hostname algorithm attached to the shared resource model.

Repository-wide Rust symbol and caller inspection finds only the Resources unit
tests calling this particular method. The historical `get_tls_by_host` caller
mentioned by review knowledge is absent from the current source. The same-named
private GRPCRoute method belongs to a different live route owner and is outside
this finding.

The dedicated Resources test block at
`tests/unit/resources/edgion_tls_test.rs:293-456` maintains this disconnected
contract. In particular, `test_double_wildcard`, `test_triple_wildcard`, and
`test_hostname_longer_with_double_wildcard` positively assert acceptance of
multi-star patterns.

Actual EdgionTls validation follows
`edgion_common::tls_validation::validate_cert`. Its `is_valid_san_pattern`
pre-pass rejects those multi-star declarations with `InvalidSanPattern`.
Actual Gateway certificate selection uses
`TlsCertMatcherSnapshot::match_sni_candidates` through canonical
`edgion_algo::matcher::rfc6125_hostname::lookup`; exact attachment lookup uses
the canonical `matches` helper. Persistent storage keys use `canonical_key`.
The supported BoringSSL callback consumes those published matcher results.
None of these inspected production paths calls the resource helper.

## Structural cost and bounded improvement

A TLS hostname-policy change currently leaves a public resource method and
helper-only tests presenting a second, incompatible interpretation, despite
having no production consumer. A future caller choosing the convenient resource
method would need to rediscover which matcher owns certificate selection.
That future misuse is a projection, not evidence of a current handshake defect.
The present maintenance cost is concrete: a separate algorithm and tests retain
semantics explicitly rejected by the live validation owner.

Remove only `EdgionTls::matches_hostname`, its private wildcard algorithm, and
their dedicated helper-only tests. Keep the remaining resource serialization,
mTLS, dimension-validation and attachment-proof tests. Keep actual matcher
behavior tests at the canonical Algo owner and actual certificate-rejection
tests at the Common validation owner.

Do not replace the method with another wrapper or extend the canonical matcher
to preserve the obsolete multi-star behavior. An Algo-delegating resource
wrapper would remove algorithm duplication, but would retain an unused public
surface and introduce different semantics for that surface without an actual
consumer requirement. Keeping the code is the alternative; it preserves the
known disconnected maintenance burden. No measured CPU, heap or throughput
improvement is claimed because the helper is off the production path.

## Preserved boundaries and future verification

Preserve RFC 6125 single-label certificate matching and the intentionally distinct
Gateway API multi-level Route hostname semantics. Preserve exact-first lookup,
case behavior, invalid-pattern rejection and `InvalidSanPattern` diagnostics.
No CRD, serialized field, source-version adapter, attachment proof, generation
index, Secret resolution, ReferenceGrant policy, mTLS policy, certificate
precedence, last-known-good behavior, cold TLS compilation, or handshake audit
ownership needs to change.

Recheck all repository callers before removal. Verify that the obsolete helper
and helper-only tests are gone while canonical Algo hostname tests, Common
invalid-pattern tests and Gateway certificate-selection wiring remain.
Retain all unrelated Resources tests in the private unit-test layout. Do not
delete other methods merely because they share the same symbol name.

## Original decisions and limits

The original
`skills/04-review/tls/regression-guards/wildcard-depth-gateway-api-vs-rfc6125.md`
explicitly records this permissive helper as dead code left untouched while
closing a purported validator/runtime matching inconsistency. That decision
correctly rejects a live matching-defect claim. This finding makes no such
claim and does not change either active hostname policy; it evaluates the
remaining disconnected architecture surface under the current unification
and pre-release rules.

The original incremental atomic TLS publication and schema/transport contracts
were also read. They require the active matcher/index/generation ownership and
resolved transport fields, which remain untouched. Existing ARCH-014,
ARCH-040, ARCH-041 and ARCH-042 findings concern different owners and causes.

Evidence consists of complete primary and Resources TLS test reads, current
caller inspection, and actual canonical matcher, validation, callback and test
sources. No external downstream Rust consumer audit, execution, benchmark,
production incident or runtime security failure is claimed. Independent
verification remains pending.
