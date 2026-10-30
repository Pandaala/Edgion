---
name: dns-owned-family-slice-convergence
description: Complete DNS family answers, owned EndpointSlice reconciliation, policy narrowing and retry boundaries.
knowledge_type: regression-guards
---

# DNS-owned family slice convergence

## Trigger and correction

ISSUE-008-P1-dns-family-removal-retains-obsolete-endpoints: a dual-stack
ExternalName answer changed to one family, but the Controller patched only the
surviving slice and marked its fingerprint applied. The obsolete ready sibling
remained selectable indefinitely. Required-family-only skip checks and Service-key
GC could not repair this state.

`services/dns_resolver/service.rs` now reconciles the complete owned family set.
The previous fingerprint is invalidated before starting PATCHes; the new
family-bearing fingerprint is applied only after desired PATCHes and obsolete
DELETEs succeed. Otherwise, a partial publication followed by DNS returning to the
previous answer could match the old fingerprint while leaving intermediate
addresses selectable. A matching fingerprint cannot skip an observed obsolete owned
sibling. Failed desired PATCHes preserve allowed last-good siblings; failed cleanup
remains retryable. The existing leader job owns this IO, not a resource handler or
second workqueue.

## Completeness and narrowing

Shared Common Family queries select the configured family. Standard All requires
both A/AAAA queries to complete: evidenced NOERROR NODATA can be empty;
timeout/SERVFAIL or exhausted CNAME resolution cannot be hidden by the other
family's success. With locked Hickory 0.26.1, accept an empty family only when
`NoRecordsFound` has NOERROR, an SOA and a matching SOA in preserved authorities.
Wire negative responses retain those authorities; the resolver's synthetic CNAME
exhaustion error does not, even when the last CNAME response contains an SOA.
Empty responses without this evidence fail conservatively; this does not mean
DNS forbids no-SOA NODATA. Revalidate the error-field distinction when upgrading
Hickory. Native infrastructure queries retain their existing semantics.
Typed HTTPDNS follows the same
logical completeness contract with bounded parsing and a shared primary deadline.
System DNS may hide partial failures; fixed/combined HTTPDNS relies on its provider
returning a complete set. Neither provides independent wire-family proof.

Same-policy query failure retains allowed last-good output. A narrower Service or
Controller policy withdraws forbidden owned families before querying, even on a
subsequent timeout. Invalid annotations query nothing and withdraw only proven
owned slices. Gateway's explicit Service family gate prevents stale ExternalName
membership from remaining selectable during delayed slice cleanup; already-started
requests and watch propagation retain their existing consistency boundaries.

Service > Controller configuration determines Controller publication; Service >
Gateway > selected global EGC determines Gateway self-resolution. Gateway/EGC
settings never rewrite shared Controller slices. Defaults are IPv4-only; all and
IPv6-only are explicit. Ordinary Service slices are unaffected.

## Ownership and retry

Deletion requires exact generated name and namespace, managed-by/synthetic labels,
Service-name label, and a `v1/Service` owner reference with the same Service name
and nonempty owner UID. An obsolete sibling owned by an earlier same-name Service
incarnation is eligible. Its own observed UID/resourceVersion are mandatory delete
preconditions. A 404 completes absence; 409 and other failures retain pending
convergence. Foreign or unproven objects are preserved, diagnosed, and never treated
as successful repair. GC reuses the same ownership rules.

## Evidence and limits

Canonical authority: [design rules](../../../00-design-rules.md), particularly
workqueue ownership, schema transport, shared mechanisms and failure policy.
Controller focused tests cover family removal, failed-delete retry, stale siblings,
policy narrowing, foreign ownership and precondition outcomes. The follow-up
three-generation regression publishes a dual-stack answer, partially writes a
replacement (either family PATCH failure or obsolete DELETE failure), then restores
the original answer and verifies actual endpoint repair before allowing skip.
Common transport tests exercise both families with terminal SOA NODATA, CNAME to
NODATA, unfinished chains with and without SOA, loops and ambiguous empty/referral
responses. These regressions correct the initial implementation's stale-fingerprint
and synthetic-NOERROR gaps found during the second independent review.
Local dynamic tests are distinct from actual
Kubernetes leader publication. The Kubernetes canonical preflight reached duplicate
fixture ownership validation and stopped before deployment; no live Kubernetes
family convergence or IPv6 result is claimed. The final workspace checks and
independent review passed, as did local DnsResolutionFlap with actual IPv4/IPv6
backend identity and strict cache-residue validation. These local results do not
replace the pending live Kubernetes publication acceptance.

Re-evaluate on naming/label/owner changes, provider result semantics, publication
fingerprint changes, new family modes, or attempts to filter ordinary Service
membership under this DNS-only policy.
