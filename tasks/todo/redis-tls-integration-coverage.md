# Add Successful Redis TLS Integration Coverage

## Meta
- Status: pending
- Severity: medium
- Category: integration-testing

## Evidence
Existing integration coverage proves plaintext/auth behavior and TLS rejection against a plaintext server, but not a successful Redis TLS handshake.

## Why separate
Real TLS behavior requires a dedicated integration-test change and review.

## Candidate next step
Add isolated coverage for scheme-only `rediss://`, default trust, custom CA, and mTLS, with bounded readiness polling.
