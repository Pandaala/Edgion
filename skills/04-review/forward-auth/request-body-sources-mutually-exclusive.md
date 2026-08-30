---
name: forward-auth-request-body-sources-mutually-exclusive
description: Use when reviewing ForwardAuth body-source findings; covers why forwardBody and request.body cannot be configured together.
status: fixed
finding_id: BODY-009
closed: 2026-07-27
---

# ForwardAuth Request Body Sources Are Mutually Exclusive — FIXED

## Conclusion

`ForwardAuthConfig::validation_error()` rejects `forwardBody: true` when
`request.body` is also configured. The incoming committed request body and the
configured outbound body template are two complete, mutually exclusive sources
of outbound bytes and body metadata.

Fix suggestions that assign precedence to one source or silently ignore the
other are **not accepted**.

## Core Rationale

**1. Each source owns the complete outbound body**

`request.body` renders configured JSON, form, or raw bytes and may derive a
`Content-Type`. `forwardBody` replaces the outbound bytes with the current
committed incoming request body and synchronizes framing. Combining them could
send bytes from one source with media-type metadata from the other.

**2. Shared validation preserves Controller and Gateway parity**

Both the Controller schema validation and the Gateway plugin compiler route
through `ForwardAuthConfig::validation_error()`. Rejecting the conflict there
keeps status reporting and data-plane enforcement aligned.

## Fix Suggestions Not Accepted

- Prefer `forwardBody` and ignore `request.body` — hides an operator mistake and
  can preserve metadata derived from the ignored template.
- Prefer `request.body` and ignore `forwardBody` — silently disables the
  operator's request-body forwarding intent.
- Resolve the conflict only in the Gateway runtime — would let Controller
  status validation disagree with data-plane compilation.

## Re-evaluation Triggers

Re-open this decision only if:

- ForwardAuth gains an explicit API that separates byte replacement from body
  metadata selection and defines their composition.
- The shared Controller/Gateway validation contract is replaced by another
  single canonical enforcement point.

## Reference Cases

- `BODY-009`, closed 2026-07-27.
- `edgion-resources/src/resources/edgion_plugins/plugin_configs/forward_auth.rs`
- `docs/en/user-guide/edgion-plugins/forward-auth.md`
