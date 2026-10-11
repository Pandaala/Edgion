---
id: ARCH-007
status: candidate
kind: architecture-debt
priority: P2
primary_partition: PART-350
independent_verification: confirmed
---

# Match mTLS conditions through borrowed certificate candidates

## Current ownership and concrete cost

`plugins/runtime/conditions/extract.rs` obtains an owned certificate through
`PluginSession::client_cert_info`. The production adapter clones the stored
`ClientCertInfo`, including subject, CN, every SAN and fingerprint, although
`RequestInfo::client_cert_info` already exposes a borrowed record. Extraction
then creates owned field candidates. Typed URI/email SAN selection additionally
creates stripped strings. `conditions/evaluator.rs` immediately consumes these
candidates with an any-match predicate.

`ConditionalUpstreamResponseBodyFilter` reevaluates conditions per response chunk.
Thus an mTLS CN condition can repeatedly copy an unrelated SAN list. Adding a new
certificate field expands the complete-record copy even when the predicate needs
only CN; another SAN selector inherits eager candidate materialization. These
copies and their repeated call path are observed source facts. Workload impact
and throughput benefit have not been measured in this review.

The original live-input borrowing review records historical fixture allocations
for typed URI extraction and explicitly identifies certificate cloning as
remaining work. Those measurements are not fresh results or production estimates.

## Bounded improvement and legitimate ownership

Change the existing certificate getter to return `Option<&ClientCertInfo>` and
match borrowed subject/CN/SAN candidates lazily within synchronous evaluation.
Retain canonical typed-SAN prefix interpretation and any-match behavior. This
needs no additional session feature method, certificate cache or result cache.

The certificate exposure gate remains necessary: absent TLS, absent peer
certificate or disabled certificate exposure must yield no certificate.
`HeaderCertAuth` legitimately owns its certificate across subsequent mutable
header operations and can clone explicitly at that boundary. DSL owns VM values
and can copy the selected CN/fingerprint without cloning the complete record.

An alternative is to keep the owned getter and optimize only SAN materialization.
That reduces some allocation but retains the unrelated complete-record copy.
Caching predicate outcomes adds invalidation concerns and conflicts with live
response-chunk inputs. Borrowing the existing record is the narrower ownership
change. Actual nested mock getter lifetimes still require implementation proof.

## Invariants, risks and future validation

Preserve certificate exposure and redacted Debug, raw untyped SAN values, typed
prefix stripping, DNS classification, any-match and one final negation. Missing
candidates must retain diagnostics, including suppressing an uncompiled-regex
message when there is nothing to match. Keep borrowing local, retain live
per-chunk evaluation and body-chunk log silence. Preserve HeaderCertAuth header
behavior and DSL value ownership.

Future validation should extend extraction/evaluator cases for missing and
masked certificates, typed/untyped SANs, regex matching and negation. Focused
real-adapter allocation checks should compare CN and first-match SAN predicates
with a large unrelated SAN list. Claim workload benefit only after measurement.

## Original context and review status

Read the original `response-condition-live-input-borrowing.md`,
`condition-set-single-owner.md`, and
`oidc-jwt-claims-json-serialization-opt-in.md` under `skills/04-review`. The
historical reduction task is referenced by the first page. Shared condition
ownership remains canonical; JWT parsing, serialization and caching are excluded.
Actual HeaderCertAuth/DSL consumers and extraction/evaluator tests were inspected.

This root concerns ownership of an existing certificate read. It is separate
from ARCH-001 capability growth, ARCH-002 body contract ownership, ARCH-004 query
source ownership, and ARCH-005 transport admission metadata. Active WAF changes
do not alter the certificate getter. Exact source/evidence bindings live in
STATE.json and the partition reports. Independent verification is pending.
