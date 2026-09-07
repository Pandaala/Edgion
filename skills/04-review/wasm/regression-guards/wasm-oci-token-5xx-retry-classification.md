---
name: wasm-oci-token-5xx-retry-classification
description: Use when reviewing Wasm OCI pull retry findings; covers why the token endpoint classifies 4xx as definitive but 5xx as retryable, matching registry GET.
status: fixed
finding_id: OCI-04
closed: 2026-07-18
knowledge_type: regression-guards
---

# Wasm OCI token endpoint 5xx must honor the retry policy — FIXED

## Conclusion

`RegistrySession::fetch_token` (`edgion-gateway/src/plugins/http/wasm/oci/mod.rs`)
formerly classified every non-success token-exchange status as
`GetError::Definitive`, so a transient 5xx from the authorization service made
the outer `RegistrySession::get` retry loop exit on attempt one even when
`wasm_oci_pull_attempts > 1`. The token endpoint now classifies statuses exactly
like a registry GET: **4xx definitive, 5xx retryable**. Fixed 2026-07-18.

Re-reports of the "token 5xx not retried / private pull fails immediately on a
flaky token service / classification inconsistent with transport + JSON-decode
retryability" form are **not accepted**.

## Core Rationale

**1. The retry contract is documented and applies to token requests**

`skills/02-features/02-config/01-gateway-config.md` (`wasm_oci_pull_attempts` row)
promises "transport errors and `5xx` retry, `4xx` never does" for OCI pull
requests — manifest, token, and blob alike. The old `fetch_token` broke that
promise for the token hop only.

**2. Internal consistency inside `fetch_token`**

Transport errors (`req.send()` failure) and token JSON-decode failures were
already `Retryable`; only the HTTP status branch was hard-`Definitive`. Making
5xx retryable removes the anomaly and aligns with the identical 4xx/5xx split in
`get_authed` for normal registry GETs.

**3. Negative-cache amplification made the bug worse than a single failed pull**

A failed Wasm load is negative-cached by `WasmVmManager`, so an immediate,
non-retried token-service blip could hold the plugin in a fail-open/fail-close
state for the whole negative-cache window even after the service recovered.

## Fix Suggestions Not Accepted

- "Retry all non-success token statuses (including 4xx)" — a 401/403 is a real
  auth failure (bad or missing credentials); retrying it wastes attempts and
  delays the definitive error. 4xx must stay definitive.
- "Log the token response body to aid debugging" — the error message
  deliberately omits the response body and the token to avoid leaking secrets.
- "Add a separate token-only retry knob" — token requests share the single
  `wasm_oci_pull_attempts` budget by design (Istio-agent
  `WASM_HTTP_REQUEST_MAX_RETRIES` parity); a second knob is unwarranted.

## Re-evaluation Triggers

Re-open only if: the retry policy is intentionally changed so token requests get
a distinct budget; or a specific 4xx (e.g. 429 with `Retry-After`) is later
decided to be retryable for token exchange (it currently is not).

## Reference Cases

- Finding `OCI-04` (review-wasm-issues), fixed 2026-07-18.
- `edgion-gateway/src/plugins/http/wasm/oci/mod.rs` — `fetch_token` status
  classification; `get_authed` for the parallel registry-GET split.
- Regression tests (same file): `token_endpoint_5xx_retries_and_then_succeeds`,
  `token_endpoint_403_is_definitive_no_retry`.
- Related wasm-oci decision: [`wasm-module-cache-key-runtime-config.md`](wasm-module-cache-key-runtime-config.md).

## Resolution references

- `OCI-04` — 2026-07-18.
