# D0116 — TLS Log Sensitive Mode

- Status: pending
- Severity: medium
- Discovered in: B0206 passthrough analysis
- Canonical task: [Apply sensitive mode to TLS log surfaces](../../../todo/apply-sensitive-mode-to-tls-log-surfaces.md)

Global TLS context serialization and both per-listener JSON serializers emit
SNI without the configured masking required by logging policy. Coordinate the
two serializers and context field rules rather than fixing only passthrough.
Additional B0207 evidence includes unmasked `TlsConnMeta.sni` in the SSL
handshake log; the canonical task covers this output as well.
