---
name: secret-header-values-must-be-sensitive
description: Use when reviewing code that materializes a resolved secret into an HTTP header value.
status: fixed
finding_id: CI-R2
closed: 2026-08-27
knowledge_type: mandatory-design
---

# Secret Header Values Must Be Sensitive — FIXED

## Conclusion

Any `http::HeaderValue` constructed from resolved secret material must call
`set_sensitive(true)` immediately after parsing and before it crosses a storage, clone, or
transport boundary. Redacting only the owning struct's `Debug` implementation is not sufficient.

Fix suggestions that rely on the header name to imply sensitivity are **not accepted**.

## Core Rationale

**1. Header names do not set the value metadata**

Neither `http` nor `h2` automatically marks `Authorization` or custom credential headers as
sensitive. The component that materializes the secret owns this classification because it knows
the value's provenance.

**2. The metadata protects structured debug output**

`HeaderValue` renders a sensitive value as `Sensitive`. Its clone preserves the flag, allowing
the protection to survive an `Arc`, request-scoped deferred storage, and insertion into Pingora's
attempt-local `RequestHeader`.

**3. HTTP/2 uses the flag for never-indexed encoding**

The `h2` encoder checks `HeaderValue::is_sensitive()` and selects HPACK's literal-never-indexed
representation. This keeps credential values out of the connection's dynamic compression table.

## Fix Suggestions Not Accepted

- Infer sensitivity from `Authorization` only — custom credential header names are supported and
  require the same protection.
- Depend solely on a container's custom `Debug` implementation — the value later enters other
  structured request types.
- Treat the flag as encryption or memory zeroization — it is metadata consumed by cooperative
  debug and protocol implementations.

## Re-evaluation Triggers

Re-open this decision only if:

- a new credential materialization path creates a fresh `HeaderValue` without copying the flag;
- the HTTP/2 implementation stops honoring `HeaderValue::is_sensitive()`; or
- a header is serialized to raw bytes before an observability path, where the metadata can no
  longer provide redaction.

The last case requires removing or redacting the raw-byte observability path itself; CI-R3 tracks
the known Pingora HTTP/1 instance.

## Reference Cases

D0036 extends this boundary through `PluginSession::set_request_header_value`
and the Pingora adapter. Authentication plugins use the canonical
`common::auth_common::set_sensitive_request_header` helper when producing tokens,
identity, claims, metadata, or cookies. The typed setter preserves replacement and
deferred-mutation semantics without converting the value back into a String.

Outbound `forward_all` copies typed values, preserving multiple values and their
sensitivity while retaining the existing UTF-8 and hop-by-hop filtering. Rendered
origin/custom headers are conservatively marked sensitive because their String
collection no longer carries secret/template provenance; reconstructed cookies
are sensitive too. This can reduce HTTP/2 dynamic-table compression for ordinary
rendered headers. It does not add a new value allocation or classify arbitrary
incoming headers as secret based on their names.

Elasticsearch Basic/APIKey/Bearer headers and the CLI bearer header carry the same
metadata. Regression coverage checks exact bytes, custom header names, clones,
duplicate forwarding, and the real adapter's deferred overwrite behavior.

- CI-R2 security review, 2026-08-27
- `edgion-gateway/src/plugins/http/credential_injector/plugin.rs`
- `edgion-gateway/src/ctx.rs`

## Resolution references

- `CI-R2` — 2026-08-27.
