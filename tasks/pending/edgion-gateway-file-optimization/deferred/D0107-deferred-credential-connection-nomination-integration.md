# D0107 — Deferred-credential Connection Nomination Integration

- Status: pending
- Severity: high (credential forwarding regression coverage)
- Discovered in: B0189 HTTP upstream-request filter analysis
- Canonical task: [Add deferred-credential Connection nomination integration coverage](../../../todo/add-deferred-credential-connection-nomination-integration.md)

A later integration-test-authorized change should exercise the real H1 chain:
CredentialInjector, a later request plugin that nominates the credential header
through `Connection`, Pingora sanitization, and the upstream request hook. Assert
fail-close, zero backend receipt, and no endpoint outlier penalty. B0189 must not
modify the frozen existing integration suite.
