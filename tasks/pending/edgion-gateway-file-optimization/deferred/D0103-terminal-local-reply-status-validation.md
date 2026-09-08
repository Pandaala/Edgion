# D0103 — Terminal Local-reply Status Validation

- Status: pending
- Severity: high (fail-close correctness)
- Discovered in: B0186 HTTP response-filter analysis
- Canonical task: [Unify terminal local-reply status validation](../../../todo/unify-terminal-local-reply-status-validation.md)

Native WAF and several request-stage plugin schemas admit informational status
codes for values later used as terminal local replies. Request-stage direct
writers can also treat a failed final-writer claim as a no-op. Audit every
producer and consumer, define one canonical 200–599 terminal-status contract,
and preserve transport-confirmed 101 as a separate protocol boundary. B0186
fixes only the proven response-stage fall-through in its frozen local scope.
