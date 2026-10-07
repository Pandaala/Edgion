---
name: httpdns-prepared-runtime
description: Review HTTPDNS configuration-time preparation, reusable clients, fallback publication, and certificate rotation.
knowledge_type: regression-guards
---

# Prepared HTTPDNS configuration generations

## Conclusion and scope

Common's ResolverSpec owns an immutable prepared HTTPDNS handle. Preparation
constructs the canonical outbound client and performs TLS configuration auditing;
resolution only queries that handle or runs its configured fallback. Gateway
ExternalName and AI external backend lookups consume the same prepared core.
No client construction, TLS policy resolution, or tracing belongs to a query.

Gateway publishes HTTPDNS in the existing typed LinkSys runtime table. All
provider dispatch is synchronous configuration work under the source-event lock
before source publication. Rebuilds, admission retries, and synchronous OTLP
shutdown mutation share that lock; no dispatch sequence guard is needed.
Actual provider connection workers remain asynchronous. There is no new global client cache,
HTTPDNS background worker, or second runtime lookup authority.

Unlike Webhook's ordinary last-good replacement, HTTPDNS publishes the latest
configuration's preparation outcome even when TLS material is missing or the
connection configuration is invalid. That outcome retains exactly the configured
fallback. A missing/wrong-type reference keeps the established System behavior;
an existing HTTPDNS with fallback:none must never be treated as a missing ref
because preparation failed. HTTPDNS is exempt from the persistent-provider cap,
like Webhook; idle HTTP connections have explicit native pool bounds.

## Rationale and evidence

Canonical authority: [design rules](../../../00-design-rules.md), especially
binary-owned observability, canonical outbound HTTP/TLS/egress, schema transport,
and joint publication. Ownership follows
[the single provider table](../intentional-design/provider-owned-runtime-and-simple-removal.md).

The original trigger was Common dns::resolve constructing ReqwestHttpDnsFetcher
for every HTTPDNS query. Gateway cache misses and AI endpoint selection reached
that constructor, whose canonical TLS resolver could emit repeated WARN records.
Preparing immutable handles removes both repeated construction and query-time
audit without suppressing configuration diagnostics.

Address-cache identity is computed with admin redaction. It never retains
resolved certificate or key bytes and does not change for a Secret-only rotation.
Client reuse uses a separate private identity including consumed TLS material;
resourceVersion alone cannot override a changed material identity. Global TLS
rebuilds force replacement. Deletion, type changes and empty full sync withdraw
current lookup; previously borrowed handles follow normal completion semantics.

Controller background resolution keeps preparation in its existing per-Service
lifecycle state, separate from the last successfully applied EndpointSlice
fingerprint. Failed preparation is retained before querying, so repeated failures
do not reconstruct/audit each tick. Desired-Service pruning releases these
handles. Credential acquisition prepares at its own cold acquisition boundary.

The reference-contract implementation retains actual LinkSys owners in the opaque
HttpDnsCandidate alongside prepared reference topology and one captured EGC global
TLS snapshot. Incoming source identity and active local pairing proof precede reuse
in both Gateway runtime publication and Controller ServiceDnsState. Enabled TLS
also checks the independently inherited axes. Absent/disabled TLS retains native
system trust/no client and does not consume global TLS; its actual source identity
is still checked. Only another provider returns no candidate: malformed existing
HTTPDNS preserves its authored fallback as a failed generation.

HTTPDNS and Webhook use the same Common inherited-axis fingerprint encoder and
pure active-local inheritance check. Local verify, entire validation/CA, and client
overrides mask the corresponding global inputs before encoding. Changing a masked
global axis keeps the private client identity; an unmasked change or failed
inherited proof prevents ready reuse. Address-cache identity remains separate.

Comparison performs no PEM parsing or TLS audit. A ready fetcher cannot be reused
under failed authority even for equal resourceVersion/policy/material bytes. A
matching failed generation has no client to authorize and retains its fallback
without repeated preparation; repaired source identity/proof triggers recovery.
Preparation reborrows the same immutable actual owner and captured global snapshot.
The canonical HTTP factory's owning-selection entry rejects raw TLS substitutions
and shares existing native TLS application and configuration auditing. Query config
copies only query fields and never clones connection/Secret material to discard.
Public direct fetcher construction validates the same full actual-owner candidate.
Other providers, plugin transports and reference-annotation ownership remain open
under tasks/pending/resource-reference-contract/; this is not a full-task verdict.

## Alternatives and limits

No retry/active-health/rate-limit engine is added: the existing explicit DNS
fallback owns failure handling. System bootstrap, disabled proxies, denied
redirects, endpoint classification, 256-IP parsing, and a 64 KiB hard response
ceiling remain. maxResponseBytes may only tighten the response bound. Connection
settings are validated at schema admission and actual runtime preparation.

Controller's raw Kubernetes LinkSys read still lacks resolved TLS Secret fields;
custom CA/mTLS configurations requiring them retain the existing fallback
limitation. This correction does not add Controller Secret acquisition, change
Gateway API routing contracts, or forcibly revoke already borrowed handles.
Idle-pool limits bound retained idle connections, not concurrent active queries.

## Verification and re-evaluation

During the reference migration, fresh Common library tests passed 350 with four
existing real-Etcd tests ignored, eight Common compile-fail doctests passed, and
Common/Controller/Gateway library checking passed. Controller DNS tests passed 11,
Gateway store tests passed 37 and the existing real HTTPDNS transport tests passed
14. These cover both incoming source/pairing replay gates, repeated failed reuse,
repair, direct actual-owner construction and comparison-only zero audit. Layout
and reference-contract drift checks passed; agent-doc validation still has three
current-HEAD dangling gRPC task links tracked in the reference task. These scoped
results do not establish live conformance, all-provider runtime coverage or
whole-task completion.

Fresh verification on 2026-10-05 passed independent Astra plan and final code/
knowledge review. Focused tests passed: Common DNS 59, real HTTP/DNS transport 14,
Resources HTTPDNS 11, Controller DNS service 9, Gateway LinkSys 300, and Gateway
store statistics 2. Shared Common LinkSys/TLS tests additionally passed 120, with
four pre-existing real-Etcd tests ignored. CRD and plugin-condition guards passed
19 and 8 tests. These cover repeated-query connection reuse and zero query-time
TLS audit, failed preparation fallback, redacted versus material identity,
Secret/global changes, source ordering, lifecycle cleanup, transport bounds,
cancellation, and Controller failure-state reuse.

Workspace format/check/Clippy and required agent-doc, SSA, metrics, Pingora source,
Gateway API transform and unit-test-layout guards passed. Gateway tracing and
English-only guards passed. The standard integration preparation rebuilt the
runtime/test artifacts before execution. DNS baseline passed 4/4; DnsResolutionFlap
passed all eight failure/recovery phases, including body-cap and invalid-connection
updates. The final strict cache-residue validator passed after wiping 50 resources
with zero deletion errors, and owned processes were stopped. The successful run
is recorded under `tmp/integration_testing/testing_20261005_234735/` and
`tmp/httpdns-runtime-audit/`. Kubernetes fixtures and runner/parity/ownership
regressions were validated locally; no Kubernetes cluster run is claimed.

Its dedicated GatewayClass/EGC permits the local backend and requires HTTP 200
in working phases, so an unrelated loopback rejection cannot mask a missing cap.
Load its initial listener fixture before starting the shared Gateway.

Revisit whenever preparation, source/runtime ordering, TLS defaults, material
identity, cap admission, fallback, bootstrap, or Controller state ownership changes.

## Resolution references

- prepare-httpdns-runtime-and-config-audit (Common quality batch B0083):
  confirmed performance/observability defect; user authorized prepared clients
  and connection configuration, with independent Astra plan/code review.
