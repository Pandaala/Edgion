# D0123 — Gateway Coalesced Add Generation

- Status: pending
- Severity: medium
- Discovered in: B0218 runtime handler analysis
- Canonical task: [Replace Gateway generations on coalesced add](../../../todo/replace-gateway-generations-on-coalesced-add.md)

A compressed delete/add for an existing Gateway produces a final add entry.
GatewayHandler updates backend identity, then add_gateway rejects the existing
key, retaining the old Gateway used to rebuild PGIS and TLS matching. This is a
persistent generation divergence, separate from D0003's omitted full-set identities.
