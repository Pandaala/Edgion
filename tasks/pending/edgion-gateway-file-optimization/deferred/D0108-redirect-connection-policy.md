# D0108 — Redirect Connection Policy

- Status: needs-discussion
- Severity: low
- Discovered in: B0194 wire verification
- Canonical task: [Clarify redirect connection policy](../../../todo/clarify-redirect-connection-policy.md)

Independent diagnosis confirmed that the dedicated redirect handler inserts
`Connection: close` but does not disable Pingora keepalive. The pinned H1 writer
overwrites that header from session policy, so ordinary HTTP/1.1 requests remain
keepalive. This behavior predates B0194. Decide the intended connection policy
separately; B0194 preserves production behavior and tests closure only when the
client requests it.
