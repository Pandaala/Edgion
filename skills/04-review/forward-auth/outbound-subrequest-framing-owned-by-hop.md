---
name: outbound-subrequest-framing-owned-by-hop
description: Use when reviewing plugin-created outbound HTTP requests that forward inbound headers; covers Content-Length and Transfer-Encoding ownership.
status: fixed
finding_id: FA-BODY-001
closed: 2026-07-27
---

# Outbound Subrequest Framing Is Owned by Its HTTP Hop — FIXED

## Conclusion

Plugin-created outbound requests must not inherit `Content-Length` or
`Transfer-Encoding` from the incoming request. The shared outbound builder removes both
headers and derives `Content-Length` only from the final buffered outbound body.

Fix suggestions that preserve incoming framing for an outbound request with an unchanged
method are **not accepted**.

## Core Rationale

**1. Framing describes one HTTP message**

An inbound request and a ForwardAuth or LinkSys webhook request are different messages. The
plugin may change the method, path, and body independently. Copying an inbound
`Content-Length` to a bodyless outbound request makes an HTTP/1.1 server wait for bytes that
will never arrive.

**2. The final body is the only valid length source**

`sync_outbound_body_framing` runs after configured bodies, forwarded-body overrides, and
extra headers are applied. It removes stale framing and sets the exact length for known byte
bodies. A truly bodyless request keeps no explicit framing.

**3. Validation and runtime enforcement are both required**

`OutboundRequestSpec` rejects explicit framing in `origin` and `custom`, while the gateway
still strips it before sending. This keeps invalid configuration visible without trusting
every runtime caller to have passed admission.

## Fix Suggestions Not Accepted

- Preserve inbound framing when the method is unchanged — method equality does not imply body equality.
- Convert every bodyless outbound request to `Content-Length: 0` — absence already expresses no body and avoids imposing transport-specific formatting.
- Let operators configure framing headers — configured values can diverge from the final body after overrides.

## Re-evaluation Triggers

Re-open this decision only if:

- The shared outbound path gains a streaming request body whose length is not known in advance.
- A new HTTP client bypasses the shared outbound builder and needs equivalent framing enforcement.

## Reference Cases

- `FA-BODY-001`: ForwardAuth `basic_post_method` timed out after forwarding inbound
  `Content-Length: 9` to a bodyless `GET /verify`.
- `edgion-gateway/src/plugins/http/common/outbound/request.rs`
- `edgion-resources/src/resources/common/outbound_request.rs`
- `docs/en/user-guide/edgion-plugins/forward-auth.md`
