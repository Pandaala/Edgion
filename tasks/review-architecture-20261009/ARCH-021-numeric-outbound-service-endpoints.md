---
id: ARCH-021
status: candidate
kind: architecture-debt
priority: P3
primary_partition: PART-253
independent_verification: confirmed
---

# Keep ready Service endpoints numeric across the shared outbound boundary

## Existing owner and consumer path

endpoint_slice/ep_slice_store.rs ready_endpoints_for_port formats canonical ready
sockets into Vec<String>. Its two production consumers are shared outbound
ResolvedTarget::pick_url and AnchorPinnedResolver::addrs. The target formats all
ready endpoints before choosing one URL. The pinned resolver parses every string
back into SocketAddr, safety-filters and rotates the numeric vector. Managed client
preparation installs that resolver for Service-pool authority targets.

Each Service-backed outbound URL pick allocates strings for the pool to select
one address. Authority-driven new connections additionally serialize and parse
the whole pool. An address representation change must coordinate discovery
formatting, URL selection and parsing even though source and dial use numeric
sockets. Current behavior is correct; no throughput measurement was performed.

## Bounded improvement and alternatives

Return numeric SocketAddr values from the existing facade, or capture a section
view allowing equivalent numeric access. Format only the chosen socket at the
URL boundary. The pinned resolver safety-checks and rotates numeric addresses
directly. Do not add another endpoint cache/index, compatibility alias or general
target abstraction. An owned numeric vector may remain appropriate for rotation;
borrowed geometry is optional, not a requirement to broaden lifetime coupling.

Current ready membership, real EndpointSlice serving ports, independent cursor
rotation and correct bracketed IPv6 output are necessary consumer semantics.
The string round trip does not implement those policies.

## Invariants and future checks

Preserve checked Service port selection, one loaded discovery generation,
readiness/order, all-or-empty behavior for unexpected non-inet slots, invalid/
absent/unavailable failure, no DNS fallback, authority-independent pinned lookup,
unsafe-address filtering before rotation and cursor behavior. Native selection
and active-health identities stay unchanged.

Adapt facade serving-port/readiness/IPv6/reload and outbound rotation/pinning/
unsafe-address tests. Verify URL picking formats one socket and pinning performs
no string parsing. Keep canonical final safety classification.

## Authority and limits

The analyst fully read all discovery primaries and both real outbound consumers,
with relevant prepared-client wiring/tests, unification registry and original
named-port and resolved-target-no-compatibility-alias records. Superseded
webhook-extract-ready-ips-ipv6-brackets requires correct output, which numeric
SocketAddr formatting still provides at the URL boundary. Static source review;
independent verification pending.
