---
name: wasm-artifact-response-streaming-cap
description: Use when reviewing findings that wasm OCI manifest/blob, token-endpoint, or url-module response bodies are buffered whole before a size check (OOM/memory-amplification risk); covers the streaming-cap fix.
status: fixed
finding_id: oci-02
closed: 2026-07-18
knowledge_type: regression-guards
---

# Wasm artifact response bodies buffered before the size check — FIXED (streaming cap)

## Conclusion

The wasm-module acquisition paths previously buffered the **entire** HTTP
response into memory and only then compared its length against a cap (or, for
the token and url-module bodies, applied no cap at all). A response with no
`Content-Length`, a falsely-small `Content-Length`, or chunked transfer could
therefore make `reqwest` allocate an arbitrarily large body before the nominal
4 MiB / 64 MiB limit rejected it — a memory-amplification / OOM surface in the
gateway data-plane process (module loads run during preload / config apply /
lazy first-request load). This is **fixed**: all four paths now stream the body
through the shared capped helper `stream_body_capped`, so peak host allocation
is bounded to the cap plus one chunk regardless of what the peer sends.

Re-reports of the "response limit applies only after full buffering" /
"token/url body has no cap" form are **not accepted** — verify the four call
sites below still use `stream_body_capped` before re-filing.

## Core Rationale

**1. One shared streaming helper, four call sites.**
`stream_body_capped(resp, max_bytes)`
(`edgion-gateway/src/plugins/http/common/outbound/call.rs`) already backs the
Wasm HTTP callout path; it accumulates bytes chunk-by-chunk and returns
`(body, truncated)` where `truncated == true` means the body would exceed the
cap. It counts **actually-streamed bytes**, so `Content-Length` truthfulness is
irrelevant. The fix wires it into:

- `oci/mod.rs` `get_authed` — manifest (`MAX_MANIFEST_BYTES`, 4 MiB) and blob
  (`MAX_BLOB_BYTES`, 64 MiB), replacing `resp.bytes()` + post-length check.
- `oci/mod.rs` `fetch_token` — token JSON, new `MAX_TOKEN_BYTES` (64 KiB),
  replacing an uncapped `resp.json()`; the bounded bytes are then parsed with
  `serde_json::from_slice`.
- `vm_manager.rs` `fetch_url_bytes` — `source.url` module, new
  `MAX_URL_MODULE_BYTES` (64 MiB, mirrors the OCI blob cap), replacing an
  uncapped `resp.bytes()`.

Each of the three OCI/url sites also does a **Content-Length fast-reject** first
(`resp.content_length().is_some_and(|cl| cl > cap)`), mirroring the sibling
`outbound_call` precedent (`call.rs`): an honestly-oversized length fails before
the body stream is even opened. This is a fast path, **not** the load-bearing
guard — the streaming counter below it still catches absent / false / chunked
lengths.

**2. Boundary parity with the old check.** `stream_body_capped` returns
`truncated == false` when the body is **exactly** `max_bytes`, matching the old
`bytes.len() > max_bytes` (strictly-greater) rejection. Digest verification
still runs over the bounded accumulated bytes, so the security check is
unchanged.

**3. Scope of the concurrency evidence.** The historical fix addressed bytes
per HTTP body. `url_fetch_permits()` bounds active request attempts and releases
its permit before post-download processing; it does not by itself bound all
retained downloads, digest work, extraction, or compilation. D0074 adds a
separate OCI pipeline lease; see the
[bounded artifact-cache decision](../../wasm/intentional-design/bounded-oci-artifact-memory-cache.md).
Do not use the historical four-request limit as evidence of a whole-pipeline
memory bound.

**4. Test coverage & its limit.** The regression tests serve honestly-oversized
bodies, so they exercise the Content-Length fast-reject. The **false / absent /
chunked** Content-Length path (the streaming counter itself) is **not**
unit-testable with the `wiremock` + `hyper` stack: hyper panics on a
Content-Length/body-length mismatch (`h1/role.rs`), so a mock cannot emit a
false-small length, and wiremock has no chunked/streaming body. This is the same
limitation the pre-existing `response_too_large_returns_error` test (`call.rs`)
lives with. The streaming counter's truncation logic is shared, small, and
carried by the production callout path; do **not** re-file "add a false-CL
streaming test" as an actionable gap unless the test stack gains a way to send
one.

## Fix Suggestions Not Accepted

- "Give the url module its own operator-tunable size limit" — no operator need;
  a fixed cap matching the OCI blob cap (64 MiB, well above single-digit-MiB
  real modules) is sufficient and keeps the surface small.

## Re-evaluation Triggers

Re-open only if:

- A new artifact-acquisition path is added that reads a response body directly
  (`resp.bytes()` / `resp.json()`) instead of via `stream_body_capped`.
- `stream_body_capped` is refactored such that its truncation boundary changes
  (would break the exactly-`max_bytes` parity above).

## Reference Cases

- Finding oci-02 (`tasks/review-wasm-issues/`), closed 2026-07-18.
- Source: `edgion-gateway/src/plugins/http/wasm/oci/mod.rs` (`get_authed`,
  `fetch_token`, `MAX_TOKEN_BYTES`) and
  `edgion-gateway/src/plugins/http/wasm/vm_manager.rs` (`fetch_url_bytes`,
  `MAX_URL_MODULE_BYTES`); helper `stream_body_capped` in
  `edgion-gateway/src/plugins/http/common/outbound/call.rs`.
- Regression tests: `oversized_manifest_is_rejected_at_the_cap`,
  `oversized_token_response_is_rejected_at_the_cap` (oci/mod.rs),
  `url_fetch_rejects_oversized_body_at_the_cap` (vm_manager.rs).

## Resolution references

- `oci-02` — 2026-07-18.
