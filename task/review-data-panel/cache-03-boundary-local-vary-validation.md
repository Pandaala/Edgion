# Evaluate boundary-local response-cache Vary validation reuse

Status: deferred for user decision. Priority: P3. Type: performance investigation.
Inspected baseline: `02f6b77f6`. No runtime change.

## Evidence

The canonical `response_cache::key::VaryCertificate::parse` validates all Vary
field lines, hashes normalized names, deduplicates and sorts the digests. It uses
fixed storage for 16 unique names and limits aggregate input to MAX_METADATA
(16 KiB). It does not allocate a heap certificate. Header-name hashing already
uses bounded stack chunks; this task concerns repeated parsing, not that updater.

Successful paths in `response_cache/storage.rs` can parse the same immutable
response header twice within each of these separate boundaries:

| Boundary | First parse | Second parse |
| --- | --- | --- |
| `Pending::create` | Parse Vary and check request-certificate permission | `Freshness::from_headers` calls the supported-header validator |
| `Entry::decode` | `self.vary.matches` parses and compares the stored certificate | `policy::valid_origin_headers` shares the origin parser and supported-header validator |
| `retain_header_charge` | `matches_contract` calls `policy::supported_headers` | `permitted_vary` checks request-certificate permission |
| `seal_pending_headers` | `matches_contract` calls `policy::supported_headers` | `permitted_vary` checks the chosen header |
| `prepare_publication` | `matches_final_transport_contract` calls the supported-header validator | `permitted_vary` checks the final header |

The policy helper discards the parsed Vary result after checking validity.
Short-circuit failures can avoid later work, so these are successful-path duplicate
parses, not an unconditional request count. Capture, replay, seal and publication
remain distinct validation boundaries even when their current headers happen to
match. Missing Vary is also valid and cheap; benefit depends on traffic shape.

The existing single-pass Cache-Control lifetime extraction shares one parse
inside freshness construction, while intentionally validating each boundary
separately. The current metadata contract helper similarly shares only one
complete framing pass within a call. Neither change authorizes caching a proof
across response plugins or transport mutation.

## Decision and acceptance

Decide whether measured duplicate parsing cost justifies a local validated-result
API, or retain the simpler independent calls. No CPU or throughput benchmark was
run for this candidate, and no heap-allocation reduction is claimed. Measure
absent Vary, one/many names, duplicate names, early rejections and long bounded
input before selecting the design.

A future IMPL must identify an exact allow-list and independently review ownership
and call order. Keep one canonical parser. A validation result must be usable only
with the same immutable header during that validation boundary; do not add a
boolean that bypasses validation, a global cache, or reusable request state that
can survive header mutation. Evaluate a borrowed validated view or a tightly
scoped combined result rather than copying or trusting an earlier certificate.

Preserve grammar, wildcard/empty-member rejection, case normalization, duplicate
handling, aggregate-byte and unique-name limits, digest identity and request-key
representability checks. Preserve the first stored metadata value, missing versus
empty values, complete malformed-tail rejection, supported-header policy and
existing short-circuit effects. Keep capture clock sampling and origin freshness
ownership explicit; never restart age at replay. Final transport may change only
the separately validated singleton Date under the existing contract.

Retain each boundary's outcome: identity-checked eviction and retained header
charge on replay, abandonment on a failed seal/publication, and epoch fencing.
Do not change admitted memory ownership, body copying, response streaming,
terminal ordering or observability. Tests must mutate Vary between capture, onion,
seal, replay and publication to demonstrate that a prior result cannot suppress
fresh validation; include corruption, permission denial and cancellation cases.

Canonical constraints:
`skills/04-review/http-body/regression-guards/bounded-response-cache-capture.md`.

Fresh unchanged-code baseline verification passed seven cache-key, 21 storage,
seven lifecycle and four policy tests (39 total), formatting check, workspace
check/Clippy and agent-doc validation. Existing unrelated warnings remain. The
broader policy admission fixture in verification-01 remains unresolved; these
focused passes neither exercise nor repair that fixture. No live integration or
alternative validation-result implementation was run.

Decode validation now shares the private origin-header parser with capture rather
than constructing and discarding a Freshness value. It no longer samples the wall
clock or computes an unused age, but still invokes supported-header validation
and its Vary parser. Both Vary parses in the decode row therefore remain; this
lifetime/validation split does not select or implement the proposal above.
