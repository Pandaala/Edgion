---
name: named-port-resolve-against-originating-slice
description: Review canonical Service section port resolution, heterogeneous EndpointSlice origins, and DirectEndpoint authorization.
status: fixed
finding_id: port-resolver-heterogeneous-slice-ports
closed: 2026-07-18
knowledge_type: regression-guards
---

# Service ports resolve through immutable EndpointSlice section proofs

## Conclusion and scope

The request-path `resolve_target_port_key` rewriter is removed. The discovery coordinator
compiles each Service port section against its exact EndpointSlice serving ports and publishes
immutable slots. HTTP, TCP, TLS passthrough/termination, UDP, and request-mirror selectors
consume the section instead of retaining or rewriting an unrelated first port.
AllEndpointStatus consumes ready slots and section-scoped TLS policy as described in
[its owning rule](all-endpoint-status-tls-identity.md).

DirectEndpoint borrows `CompiledBackend::permitted_service()` from the actual
Route compiler: the same private Service key, checked port section and policy
target used by ordinary native HTTP selection, with no second classifier. The section's numeric IP/port index is
compiled from final `all_slots` only during runtime materialization. Pure normalized
geometry retains numeric sockets and originating proofs without executable wrappers
or an authorization index; exact preflight and publication share that normalization.
Explicit endpoint ports must belong to the authorized section; absent explicit ports require
a unique serving port for that IP. Missing Service/section, zero/overflow/negative backendRef
ports, unproved endpoint ports, and ambiguous IP mappings cannot authorize a direct dial.
Denied references remain excluded. No request-time raw EndpointSlice scan, key formatting,
string-to-IP parse, resource clone, or blocking lock is needed for authorization.

The selected address and the policy looked up using the same proven section name are captured
in `DirectEndpointPreset` for retries. `inheritTls: true` uses the canonical TLS peer builder;
an invalid policy rejects the override, and only an absent policy permits plaintext.
The existing explicit `inheritTls: false` opt-out remains. Endpoint-derived final dialing
rejects Unix sockets and applies the canonical loopback/link-local checks.

## Rationale and evidence

A Service port and an EndpointSlice serving port are different identities: `443 -> 8443`
must dial 8443. A request supplying a sibling port on the same IP must not gain authority
merely because that number exists elsewhere on the slice. Heterogeneous named-targetPort
rollouts legitimately publish distinct serving ports per endpoint; each section preserves
those origins rather than selecting a global minimum.

Sources:
- `edgion-gateway/src/backends/discovery/coordinator.rs`: section compilation,
  materialization, `direct_endpoint_addr`.
- `edgion-gateway/src/backends/validation.rs`: precompiled route permission and
  one-generation numeric authorization plus policy lookup.
- `edgion-gateway/src/plugins/http/direct_endpoint/plugin.rs`: captured override.
- `edgion-gateway/src/routes/http/proxy_http/pg_upstream_peer.rs`: retry peer construction.
- `edgion-gateway/src/backends/peer_builder.rs`: canonical TLS and final address boundary.

Authority: `skills/00-design-rules.md` A/B/D/F/U/V and
`skills/01-architecture/02-gateway/07-backends.md`.

## Alternatives and limits

The Controller DNS producer's successful-publication fingerprint uses the same
complete `EndpointPort` projection as its slice builder (`ARCH-098`). A Service
port name or protocol change must republish even when its UID, resolved IPs and
port numbers are unchanged; otherwise the Gateway's exact section matching can
retain no slots. AppProtocol changes also republish to refresh output metadata;
the Gateway derives section appProtocol from the Service. Sorted JSON-encoded
ports preserve optional-field distinctions and ignore port order. The producer
continues to publish Service port numbers, not targetPort, and records success
only after every desired family applies. This identity does not promise repair
of arbitrary out-of-band edits to observed slice content. Focused producer tests
exercise mixed-family repair and subsequent skip; Gateway tests cover stale and
repaired name/protocol matching alongside the existing HTTP DNS bridge control.

The user explicitly approved allowing published unready endpoints for DirectEndpoint on
2026-09-08. This supports targeted debugging and deliberately overrides normal LB eligibility;
it does not bypass route authorization or address safety. Normal selectors still enforce their
own readiness/health rules. `onInvalid: fallback` discards the direct override and selects
normally; it never dials the rejected target.

Captured requests may finish on an older discovery mapping and policy. Later requests see the
new publication; this is not revocation of already captured or dispatched requests. Gateway
fallback client identity follows the existing canonical TLS builder lifecycle.

Historical evidence: the July 18 fix used numeric origin matching in a request-time rewriter.
Its unknown-origin `min()` fallback was deterministic but did not prove authorization. That
fallback and its old scope restrictions are obsolete, not accepted current behavior. The
September task's references to two EndpointSlice read locks and deep clones also describe the
removed store implementation, not the current immutable facade.

## Verification and re-evaluation

Verified on 2026-09-08 using the supported default Gateway build:

- `cargo test -p edgion-gateway --lib backends:: -- --test-threads=1`: 249 passed.
- `cargo test -p edgion-gateway --lib direct_endpoint -- --test-threads=1`: 15 passed
  (overlaps the backend tests above).
- Existing DirectEndpoint integration suite passed before and after the dynamic scenario.
- `test_client -g --dynamic-scenario DirectEndpointPortResolution`: passed, including
  HTTP serving-port changes, sibling-port rejection, unready override, missing section,
  HTTPS with a named-section custom CA, missing-CA rejection, and recovery.
- Final shared-topology metrics, unknown-field, requeue-loop, and cache-residue checks passed;
  the scenario cleaned its resources and the test services were stopped.
- Workspace check and Clippy passed; Clippy retains existing repository warnings.
- Formatting, agent-doc consistency, SSA force, metrics inventory, Pingora source policy,
  and Gateway tracing guards passed. Independent read-only review: LGTM.

This run did not execute full workspace tests or Kubernetes integration. Historical July
integration results were not rerun by the knowledge migration and are not current evidence.

Re-evaluate when section construction/materialization changes, a consumer rewrites serving
ports after proof, DirectEndpoint eligibility changes, or address/policy capture moves.
Focused tests cover heterogeneous origins, numeric IPv6 equality, ambiguous same-IP ports,
invalid backend refs, allocation-free authorization, unready overrides, and captured generations.
The `DirectEndpointPortResolution` shared-topology scenario exercises actual HTTP/TLS dialing
and reload/rejection/recovery.

## Resolution references

- `port-resolver-heterogeneous-slice-ports`: historical real defect fixed 2026-07-18;
  originating-slice semantics preserved by the coordinator.
- `backend-port-resolution-precompiled-fail-closed`: partial prior implementation,
  remaining DirectEndpoint authorization and endpoint-address boundary corrected in this task.

## Borrowed DirectEndpoint input

DirectEndpoint reads through the canonical session `key_value` helper and passes
an input slice to synchronous resolution. Regex captures remain slices of that
input rather than temporary Strings. Composite RemoteIpAndPath still uses the
helper's owned fallback. The result owns its numeric address and retained policy,
so no session borrow crosses jump or outcome mutation. Endpoint parsing, route
permission, discovery generation, TLS policy and final address safety checks
retain their order and semantics.

Tests use real header maps and prohibit the owned getter, retain missing/invalid/
unauthorized/unsafe and locked-jump checks, and cover empty/opaque input plus a
successful regex capture. This removes temporary input copies, not all resolution
allocations; no throughput improvement is asserted.
