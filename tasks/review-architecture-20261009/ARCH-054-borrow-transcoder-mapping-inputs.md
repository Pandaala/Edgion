---
id: ARCH-054
status: candidate
kind: architecture-debt
priority: P3
partition: PART-195
analyst: /root/verify_ip_union
baseline: BASELINE-AUDIT-20261010-C.json
---

# Borrow unescaped gRPC transcoding mapping inputs

## Current ownership and concrete cost

The private `decode_path` in `edgion-resources/src/grpc_transcoding/mapping.rs:314-340` always prepares a `Vec` with capacity equal to the input length, copies the decoded bytes, and converts it into a `String`. This happens even when the input contains no percent escape.

`CompiledBinding::bind` uses it in two ways. Path captures are decoded at `:134-141`, then passed to `scalar`; string-like fields copy that temporary text into the owned JSON value needed by `BoundRequest`. The complete raw query is decoded at `:143-145` solely for validation, and the returned string is immediately discarded before the existing `form_urlencoded` parser extracts parameters.

For the existing GET `/v1/numbers?count=3` test, query validation allocates and copies `count=3` without using the resulting text. A plain string path capture similarly receives a temporary copy before the final field value is constructed. These are bounded request-path allocations, not evidence of a leak, measured latency regression or a particular RSS increase.

The Gateway request callback invokes the compiled binding directly before body capture and conversion admission. This cost therefore occurs during ordinary request mapping, independently of the protobuf/ProtoJSON conversion buffers.

## Bounded improvement and alternative

Change the existing private helper to return `Cow<'_, str>`. If the input contains no `%`, return the borrowed input immediately. Otherwise retain today's strict percent-decoding loop and owned UTF-8 conversion. Both current callers can consume the borrowed or owned text within `bind`; no borrowed data needs to escape into `BoundRequest`, and its final field values remain owned.

This removes the discarded allocation for ordinary queries and the temporary plain path-capture copy while retaining one decoding owner. It requires no public API, descriptor cache, session interface, new dependency or generic decoder framework.

A smaller alternative special-cases percent-free queries before invoking the current helper. That removes the query cost but leaves plain path captures unnecessarily copied and spreads the same classification between callers. The private Cow result handles both current uses in one place.

Escaped input still allocates under the existing implementation. Eliminating that allocation through an incremental UTF-8 validator is outside this proposal. The percent-presence check adds a scan before escaped decoding; no throughput improvement is claimed without measurement.

## Required invariants and future verification

Preserve complete raw-query validation before parameter parsing and before the unknown-query ignore policy. Malformed percent escapes and invalid decoded UTF-8 must still fail with `TranscodingError::Invalid`, including errors in fields that would otherwise be ignored.

The validation helper leaves `+` unchanged; the subsequent form parser owns `+`-to-space conversion. Do not substitute the permissive shared URI decoder or move validation to already parsed parameters. Preserve whole-query validation, empty inputs and existing rejection ordering.

Escaped path captures retain strict hexadecimal decoding and UTF-8 validation. Multi-segment captures retain encoded slash bytes with their original spelling, while single-segment captures decode those slashes. Keep path/query/body overlap checks, aliases, duplicate parameters, scalar ranges, mapping ambiguity defense, the 64 KiB input bound and the existing binding limits.

Future verification should distinguish borrowed plain input from owned escaped input, retain the existing mapping/codec regressions, and cover malformed escapes, invalid UTF-8, ignored unknown fields, plus signs, encoded delimiters and multi-segment encoded slashes. Confirm that final BoundRequest values remain valid after the original request input drops.

Descriptor admission and generation ownership, canonical normalization and ProtoJSON, request BodyStorage, private bounded unary response conversion, streaming windows, output credits, deadlines and terminal diagnostics remain unchanged. Add no request tracing, metrics or payload-bearing logs.

## Original decisions, deduplication and limits

The original transcoder admission/outcomes ledger requires independent input, structure, output and heap bounds; it does not require copying percent-free mapping inputs. The descriptor ownership guard governs actual ConfigMap entry ownership and transport material, which this private request decoder does not change.

The shared URI decoder has permissive malformed-escape and invalid-byte behavior, so it cannot replace this strict decoder. The GraphQL envelope decoder additionally translates plus signs and charges its own envelope budget; it is not the same operation.

ARCH-033 concerns an unused response callback writer. ARCH-048 concerns per-token allocation in the different permissive URI decoder. This finding concerns unconditional whole-input materialization by the transcoder's existing strict decoder and introduces no changes to either mechanism.

Source and existing test bodies were inspected statically. No tests, builds or benchmarks ran. Independent verification is pending.
