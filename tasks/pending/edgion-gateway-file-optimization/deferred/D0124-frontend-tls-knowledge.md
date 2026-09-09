# D0124 — Frontend TLS Knowledge Drift

- Status: pending
- Severity: low
- Discovered in: B0221 test analysis, supplemented by B0222 matcher analysis
- Canonical task: [Reconcile Gateway frontend TLS knowledge](../../../todo/reconcile-gateway-frontend-tls-knowledge.md)

The standalone task owns stale fallback, strict CA-source, matcher-gating and
compiled-certificate descriptions. Runtime/test behavior remains unchanged;
independent task validation is part of the B0221 review gate.
