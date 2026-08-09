# gw-19 — Provider header stripping is a fixed denylist, so unlisted edge headers still reach the client

| Field | Value |
|-------|-------|
| ID | `gw-19` |
| Disposition | `confirmed` |
| Severity | P3 |
| Dimension | Security / information disclosure |
| Location | `edgion-gateway/src/ai/anthropic/response.rs:811-848` (`strip_provider_headers`) |
| Reviewer | Streaming / translation review |
| Status | open |

## Summary

`strip_provider_headers` enumerates a fixed set of header names plus the `anthropic-`/`x-anthropic-`/`x-amzn-*`/`x-amz-request-id` prefixes; every other upstream response header is forwarded verbatim, defeating the module's own goal of not exposing provider metadata.

## Details

The function removes a literal list (`request-id`, `x-request-id`, `anthropic-request-id`, `location`, `set-cookie`, `content-encoding`, `etag`, …) and then filters `response.headers.keys()` for the Anthropic/AWS prefixes. Anything else survives — notably the CDN/edge headers the provider sits behind (`cf-ray`, `cf-cache-status`, `via`, `alt-svc`, and `server` unless the operator configured `server_header_opts`), plus any header a self-hosted Anthropic-compatible endpoint adds.

The policy is deny-listed while the surrounding design clearly aims at full provider opacity — it strips even `x-amzn-trace-id`, and the wire contract states provider bodies are never forwarded.

## Impact

A client can fingerprint the concrete upstream provider and its edge, and correlate requests via `cf-ray`, despite the gateway explicitly scrubbing every provider request-ID header it knows about. No credential or tenant data leaks; this is metadata disclosure that contradicts a stated design goal.

## Suggested fix

Invert the policy for this processor: build the outgoing header from an allow-list (`content-type`, the `retry-after` it re-inserts, and Edgion-owned headers). The response header is fully synthesized by the translator anyway, so an allow-list costs nothing.

## Notes

Argued structurally from the denylist shape; the exact set of edge headers a given provider returns was not enumerated against a live endpoint.
