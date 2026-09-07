---
name: pingora-h1-header-logs-have-an-error-floor
description: Use when reviewing Pingora HTTP/1 request-header logging or attempts to enable its upstream client/proxy targets below ERROR.
status: fixed
finding_id: CI-R3
closed: 2026-08-27
knowledge_type: regression-guards
---

# Pingora HTTP/1 Header Logs Have an Error Floor — FIXED

## Conclusion

The complete HTTP/1 request-header DEBUG/TRACE statements are inherited from upstream Pingora,
not added by Edgion. Edgion leaves the fork code unchanged and applies a separate system-log
filter that caps the two exact upstream modules at ERROR, even when `RUST_LOG` requests a more
verbose level.

Fix suggestions that redact already serialized HTTP/1 wire bytes are **not accepted**.

## Core Rationale

**1. Provenance is upstream Pingora**

Git blame attributes both the raw client-header trace and the proxy request-header debug statement
to Cloudflare Pingora's initial `0.1.0` release commit. They are not Edgion fork additions.

**2. Header sensitivity cannot protect serialized wire bytes**

`http_req_header_to_wire` copies each value's bytes without retaining
`HeaderValue::is_sensitive()`. A later raw-byte trace therefore cannot distinguish credentials,
cookies, query data, or arbitrary custom secret headers.

**3. A separate filter makes the operational policy non-bypassable**

The ordinary `EnvFilter` accepts `RUST_LOG` and remains operator-configurable. An independent
filter is ANDed with it and allows only ERROR metadata for
`pingora_core::protocols::http::v1::client` and `pingora_proxy::proxy_h1`. The filter precedes both
file and console sinks. Other Pingora targets retain normal configurability.

## Fix Suggestions Not Accepted

- Modify the inherited fork statements — the chosen project policy is to leave upstream code
  unchanged and enforce the binary's logging boundary.
- Append more directives to the operator-controlled `EnvFilter` — a competing or more-specific
  directive can reopen the target.
- Scan or redact serialized HTTP/1 bytes — arbitrary custom headers and request-target data make
  byte-level redaction incomplete.
- Set the entire `log` facade to ERROR — that would suppress useful non-Pingora dependency logs.

## Re-evaluation Triggers

Re-open this decision only if:

- a Pingora update moves a complete-header log to a different target;
- logging initialization stops applying the independent filter before every system-log sink; or
- Edgion intentionally needs sub-ERROR diagnostics from either protected module and first adds a
  metadata-only upstream logging seam.

## Reference Cases

- CI-R3 security review, 2026-08-27
- `edgion-common/src/observe/logs.rs`
- Pingora upstream release commit `87973292`

## Resolution references

- `CI-R3` — 2026-08-27.
