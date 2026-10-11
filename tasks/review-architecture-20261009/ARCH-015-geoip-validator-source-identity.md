---
id: ARCH-015
status: confirmed
kind: behavior-defect
priority: P2
primary_partition: PART-264
independent_verification: confirmed
---

# Bind persisted GeoIP validators to their update source

## Evidence and ordinary trigger

GeoIP update_url is startup configuration; changing it requires a restart.
GeoIpUpdater captures that URL, but UpdateState persists only ETag and
Last-Modified beside db_path. Loading the sidecar checks that a regular database
and active provider exist, without checking which source produced the validator.
build_request sends the recovered validator to the newly configured URL, and
handle_not_modified_response accepts 304 when a validator was sent and local
database/provider remain usable. The run loop then treats the cycle as success.

Retain a volume and db_path across a restart from URL A to URL B. A's database
and sidecar contain ETag "v1"; B legitimately uses "v1" for a different database.
B can answer 304 to that conditional GET, so A's database remains installed and
subsequent cycles keep validating it against B. No malformed server response or
hash collision is required: entity tags are scoped to representations of their
resource, not globally unique across URLs. This is a static source-derived case,
not an executed update or evidence of a live enforcement incident.

The updater proves local usability but lacks source identity for its persistent
conditional state. Restart-required configuration does not provide this proof
when the durable files survive. Original local-eligibility tests do not cover a
source change. Their valid no-sidecar mtime fallback is a separate existing policy.

## Bounded correction and alternatives

Associate the existing sidecar with an opaque identity of the configured request
source. At startup, recover conditional state only when that identity matches.
A known mismatch must suppress both stored validators and the mtime fallback
until an unconditional, structurally validated 200 from the new source swaps
successfully. Keep the old usable database as the existing serving fallback while
that fetch fails; do not invent a new readiness or traffic-denial policy.

Compute identity once at the cold updater boundary. URLs can contain credentials,
so do not persist or log plaintext URLs or Authorization values. The precise
source key and credential-dependent representation policy require implementation
review; this finding establishes the URL-change case only. Reuse the sidecar,
without another cache, registry or full-body buffer. Preserve established
same-source conditional updates and deliberate mtime recovery behavior where its
source assumptions hold. Always unconditional updates would fix the omission but
lose the existing restart/bandwidth benefit.

## Invariants and future validation

Preserve streaming byte/stall/deadline bounds, canonical outbound HTTP and egress
checks, sensitive headers/error redaction, same-filesystem rename and mmap lifetime,
structural validation, cold failure backoff and local last-good database serving.
No decoder content probe or extra operator switch is needed.

Future cases should retain one db_path across two different URLs serving different
valid fixture databases with the same ETag, then assert the first B request is
unconditional and B is installed. Pair same-source restart reuse, failed B fetch
with A still available, invalid 304 recovery and absent-sidecar mtime behavior.
Use existing integration topology for real HTTP; pure state tests need no network.

## Authority and limits

All five GeoIP primaries were fully read with lookup consumers, current updater
and provider test seams, operator restart/validator policy, lazy lookup and typed
failure contracts. No decision accepting validator reuse across changed URLs was
found. The protocol interpretation follows
[RFC 9110 ETag](https://www.rfc-editor.org/rfc/rfc9110.html#section-8.8.3) and
[If-None-Match](https://www.rfc-editor.org/rfc/rfc9110.html#section-13.1.2).
No tests/builds/updates were executed; independent verification is pending.
