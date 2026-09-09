---
name: http-call-body-truncate-to-max-string-len
description: Use when reviewing how `http.call` reports a response that exceeds its body or DSL string bound. Overflow must fail; this page records why that failure is the existing status-zero result rather than a propagated RuntimeError.
knowledge_type: intentional-design
---

# `http.call` bounded-response failures use the status-zero result

**Status**: current intentional error-shape decision. The mandatory body-overflow behavior is defined by the repository HTTP body invariant, not by this exception record.

## Current contract

The response body limit is `min(httpMaxResponseBodyBytes, max_string_len)`. A declared `Content-Length` above that limit, a stream that produces more bytes, a response-stream error, or lossy UTF-8 expansion beyond `max_string_len` fails the complete call. Failure returns the established value `{status: 0, body: "", headers: {}}`; it does not return a partial body and does not raise `RuntimeError::StringTooLong`.

The limit is enforced while streaming through the canonical outbound body helper. A body exactly at the cap succeeds only after end-of-stream proves there is no next byte. Invalid UTF-8 is converted only after a checked, allocation-free calculation proves that replacement-character expansion fits `max_string_len`.

Response header count retains the existing raw-prefix behavior: before inspecting each raw header, traversal stops if the retained Map already contains `max_map_size` names. Within that inspected raw prefix, non-UTF-8 values are skipped and a duplicate replaces an earlier value only if it is reached before the Map becomes full. Every retained lowercase name and UTF-8 value must fit `max_string_len`; an oversized retained string fails the whole call. The ignored tail is never examined.

## Intentional choice: status zero rather than RuntimeError

Overflow failure is mandatory and is not an optional policy. The intentional choice recorded here is only its script-visible shape. `http.call` has long represented transport and response-processing failures with a status-zero result so scripts can handle them beside HTTP status codes. Propagating a new runtime error for one response-limit axis would instead invoke the plugin's `DslErrorPolicy` and create a different failure surface.

Invalid arguments remain normal runtime errors, and opaque request-Body access, read, scan-budget, and request-size failures remain fixed fail-close `BodyAccess` errors. This page does not broaden the status-zero behavior to either category.

## Superseded history

The 2026-05-26 FB-03 change silently truncated the body first at `httpMaxResponseBodyBytes` and then at `max_string_len`. Its rationale was to preserve the non-raising API shape while bounding the resulting `Value::Str`. That truncation behavior is historical and superseded: the repository's plugin-initiated outbound-call invariant now requires hard streaming failure on overflow and forbids returning truncated data. The retained part of the earlier decision is status-zero failure rather than `RuntimeError`.

Redis read truncation is a separate, still-current compatibility behavior; it is not precedent for HTTP response aggregation.

## Regression guards

`edgion-gateway/src/plugins/http/dsl/tests/http_call_tests.rs` covers declared and chunked overflow, exact-cap EOF, zero-cap behavior, lossy UTF-8 expansion, response-header traversal, stream errors, and the status-zero result shape. Inline helper tests in `builtins.rs` cover checked length arithmetic and exact lossy-output preflight.

Re-evaluate only if the global `http.call` result/error contract changes. Do not reintroduce truncation, an uncapped read, a response-side `BodyStorage`, or main-upstream response aggregation.
