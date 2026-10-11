---
id: ARCH-006
status: confirmed
kind: architecture-debt
priority: P3
primary_partition: GW-COMPOSITION
analyst: /root/analyze_session
independent_verification: confirmed
---

# Resolve logical body limits before exposing the runtime limit-check API

## Current ownership and evidence

Generic gates resolve BodyRequirement.max_body_size into numeric logical_limit_bytes
when constructing BodyGatePolicy. Their request path compares numeric lengths.
`plugins/runtime/body_gate.rs:68` also exposes over_plugin_limit_len taking the
unresolved BodyRequirement; its sole production caller is body-validating HMAC.

HmacAuth request execution enters the constructed HmacVerifier, authenticates
headers/signature cheaply, ensures the body, then calls that helper. A configured
maxBodySize invokes BodyRequirement::max_body_size_bytes and parse_byte_size,
allocating lowercase text, scanning suffixes and parsing a float on each request
reaching this check. The verifier already has construction-time lifetime.

## Change scenario and cost

Configuring HMAC body validation with maxBodySize such as 1m introduces repeated
configuration parsing and a temporary allocation for authenticated body requests.
The helper makes schema-oriented input appear to be a reusable runtime-policy
API, despite the generic gate already demonstrating cold resolution ownership.
The work is source-visible and bounded; no measured throughput or large-memory
regression is claimed, and no incorrect authentication behavior was established.

## Legitimate complexity and bounded recommendation

HMAC must retain cheap authentication before capture/hash, intrinsic fail-close,
independent post-capture logical limits and non-materializing snapshot digest.
It deliberately owns admission rather than adopting the generic eager gate.

Make the shared runtime check accept an already resolved Option<u64> or equivalent
small immutable numeric policy. Resolve the HMAC limit in HmacVerifier::new using
the canonical schema parser. Preserve defensive direct-constructor invalid-config
handling and existing generic-gate secondary conditions.

This requires only the numeric boundary and HMAC constructor/call-site changes,
not a wrapper rewrite, new condition abstraction or changed digest storage. The
existing HMAC limit test mutates raw requirements after construction; configure
its input before construction rather than retaining hot-path parsing for the test.

## Invariants and future verification

Preserve cheap-auth ordering, shared physical versus post-capture logical caps,
equality allowed/greater rejected, existing fail-close and 413/408/500 mapping,
disk-backed streaming digest, conditions/ordering and canonical redacted logs.
Existing signed-digest/tampering/logical/physical-limit tests should remain with
updated construction fixtures. Verify no size parser is reachable from the runtime
logical-limit check. Benchmark only before making a throughput claim; this review
ran no tests/builds/benchmarks.

## Prior records and independent disposition

Original request-body-hash-after-cheap-auth, lazy gate materialization and prebuilt
header-HMAC verifier guards were read. The last fixed repeated verifier creation,
which is distinct from parsing sizes inside an already constructed body verifier.
Relevant task/body/HMAC/performance searches found no competing helper-resolution
issue. Independent verification remains pending and must assess value and exact
constructor/error-policy preservation.
