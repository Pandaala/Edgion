# D0109 — Session Cookie Field Integration

- Status: pending
- Severity: medium
- Discovered in: B0195 session persistence analysis
- Canonical task: [Add session cookie field integration coverage](../../../todo/add-session-cookie-field-integration.md)

Existing integration suites do not cover an affinity token in the second Cookie
field or malformed expiry refresh. Add these scenarios after integration-test
changes are authorized; keep the existing suite frozen during this campaign.
