---
name: wasm-url-source-identity-redaction
description: Use when reviewing Wasm URL-source metrics labels, module-load errors, or cold-path lifecycle logs for credential disclosure and cardinality.
knowledge_type: regression-guards
---

# Wasm URL Source Identity and Error Redaction

## Conclusion and scope

`WasmSourceSpec::metrics_label()` is the canonical observability identity for a
Wasm URL source. It emits `url:<canonical-origin>#<path-sha8>`, capped at 96
bytes. Userinfo, clear-text path, query, and fragment are absent. The digest is
computed after removing userinfo, query, and fragment, so signed-query and
userinfo credential rotations for the same origin and path retain one metric
series. Different paths remain distinguishable without disclosing path text.

Gateway URL-fetch errors retain only fixed failure categories, safe HTTP status,
and the existing byte cap. They never render the configured URL or a reqwest
error that may carry it. This applies to negative-cache state and cold-path logs
because both consume the same returned error. The real URL remains unchanged in
the fetch request and internal module cache key.

## Rationale and evidence

The mandatory sensitive-data and metric-cardinality rules in
`skills/00-design-rules.md` prohibit secret-bearing errors and uncontrolled
metric labels. Previously `metrics_label()` returned `url:{configured_url}` and
`fetch_url_bytes_with_permits()` included that value in transport, status, and
size errors. Userinfo, signed query credentials, fragments, and arbitrary path
text could therefore reach every Wasm metric and module-load WARN.

The correction is centralized at the two origins: source-label construction in
`edgion-resources/.../wasm.rs` and fetch-error construction in
`edgion-gateway/.../vm_manager.rs`. Existing metric, eviction, tick, load-state,
and request fail-mode consumers inherit the safe values without local redactors.

## Alternatives and limits

Hashing the complete URL was rejected because query-token rotation would still
create stale series. Showing a truncated path was rejected because truncation
does not make path-carried credentials safe. The origin remains readable for
operator correlation; an eight-hex digest separates paths. This identity is an
observability contract, not the module cache identity, and must not replace
`WasmSourceSpec::cache_key()` or `runtime_cache_key()`.

This guard covers configured Wasm HTTP(S) URL sources. OCI registry diagnostics,
local file paths, and guest-dispatched callout URLs have separate boundaries.

## Verification and re-evaluation

Unit coverage in `wasm.rs` uses
`url_metrics_label_is_redacted_bounded_and_generation_stable` and
`invalid_url_validation_does_not_echo_the_configured_value` to verify URL
identity and validation errors. Gateway coverage uses
`url_fetch_transport_error_does_not_expose_configured_url` and
`url_fetch_does_not_retry_4xx` to exercise transport and HTTP-status failures.
Re-evaluate if the source-label format changes, another error/log consumes
`source.url` or the cache key, or a new URL fetch implementation bypasses the
canonical label or error boundary.

## Resolution references

- `wasm-source-url-label-and-error-redaction` — task created 2026-09-07;
  confirmed and fixed 2026-09-13 from
  `tasks/todo/wasm-source-url-label-and-error-redaction.md`.
