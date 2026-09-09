# Pre-resolve L4 Listener Timeouts

## Meta

- Status: pending
- Type: refactor
- Priority: P3
- Source: Gateway campaign B0199 / D0112

## Evidence and Acceptance Criteria

TCP and TLS terminate/passthrough connection paths call the shared resource
timeout resolvers at runtime; TLS connect retries can repeat the same parsing.
The existing connect and idle bounds are present and must not be misclassified
as missing security controls.

- Investigate one immutable Gateway listener-runtime timeout shape, resolved
  during listener construction using existing resource-layer resolver functions.
- Cover TCP and both TLS modes together; do not add separate per-protocol parsers
  or caches, or change shared schema solely for this optimization.
- Preserve defaults, explicit-zero unlimited behavior, invalid-value fallback,
  listener replacement/hot-reload semantics, retries, and shared relay idle policy.
- Use a reviewed implementation plan with an exact cross-file allow-list and
  unit tests for resolution and propagation. Defer new integration coverage;
  existing integration files remain frozen during the campaign.
