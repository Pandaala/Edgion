# Prepared response-cache origin stamp components

Status: deferred optimization decision for the user. Baseline: `b6817e10b`.
No runtime change or measured performance improvement.

## Evidence

`edgion-gateway/src/response_cache/key.rs::discovery_stamp` hashes the compiled
Service target namespace, name and requested section into one digest, and the
resolved section number, name, protocol and app protocol into another. It copies
the published content fingerprint and membership identity into the resulting
stamp. `traffic_stamp` hashes the effective policy's winning_policy_key while
retaining the snapshot generation separately. These helpers use the canonical
bounded, domain-separated Builder; they do not allocate a temporary key String.

RequestState::bind calls both helpers before lookup. Its TLS report query can
also construct a discovery stamp from the TLS lookup's own discovery read.
Actual origin reporting reaches discovery_stamp through
CapturedServiceSelection::response_cache_stamp and the TLS report path, and
traffic_stamp through pg_upstream_peer's capture_service_effective_policy.
The ordinary TLS query suppresses discovery reporting, and actual peer reporting
is guarded by response-cache presence. This is not unconditional work on every
request, nor proof that every path executes the same number of hashes.

Candidate immutable owners are CompiledServiceTarget, ResolvedServiceSection and
EffectiveBackendTrafficPolicy. Their lifetimes differ: a route target can survive
discovery publication, section projections belong to discovery, and policy
snapshots carry their own generations. Existing prepared content fingerprints
and TLS profile fingerprints do not replace the other identity components.

## Decision

Retain current hashing, or prepare selected immutable digest components with their
owning publication objects. No option is selected. Preparation could reduce
repeated CPU work but adds persistent storage, construction work and cache-specific
coupling to owners also used without ResponseCache. Measure actual call frequency
and cost before choosing the scope; this is not an allocation-removal proposal.
Do not add a global digest cache or a second framing/hash implementation.

## Acceptance for a selected implementation

- Freeze an exact constructor, consumer and test allowlist after tracing every
  production construction/rebuild path. Prepared values must agree with their
  fields, including clones, reused generations and internal test fixtures.
- Preserve domain prefix, length framing, number encoding, material-limit failure
  and Option outcomes. Keep requested named versus numeric section distinctions,
  resolved section fields and their existing None/empty equivalence. Hashing only
  a Service key or port number is not an equivalent identity.
- Prepare only immutable components. Assemble complete stamps from the actual
  consumed snapshot: content fingerprint, membership, TLS/traffic generation and
  absent/invalid/effective outcomes remain live evidence. Do not merge independent
  reads, reload a store for reporting, or reuse an expected stamp as actual proof.
  Preserve expected-versus-actual mismatch rejection before fill and commit.
- Verify same-content republishing, membership changes, target/section changes,
  policy replacement/removal, TLS mapping changes and retained old requests.
  Shared named/numeric section aliases and independent target identities must
  retain their current semantics. Keep readiness and policy admission unchanged.
- Differentially compare old/new stamp bytes and failures, including bounded
  internal inputs. Measure full binding and peer reporting for cache hits,
  misses and bypasses; report publication cost, object-size/retention impact and
  non-cache-path cost. Do not infer throughput gains from hash-call counts.
- Preserve the canonical cache ownership and accounting contract, logging and
  observability. No new request tracing, metrics or dependency. Require IMPL
  plan/final independent review and repository checks before implementation.

Fresh baseline verification passed seven key, 21 storage and seven lifecycle
tests (35 total). These focused tests do not close the separate
[admission fixture issue](verification-01-cache-policy-fixture.md) or establish
complete origin-change coverage. No preparation prototype or benchmark ran.
The [cache ownership guard](../../skills/04-review/http-body/regression-guards/bounded-response-cache-capture.md)
defines actual consumed-result evidence; the
[route generation guard](../../skills/04-review/lb/regression-guards/backend-ref-selector-generation-ownership.md)
describes retained compiled owners without authorizing cross-generation reuse.
