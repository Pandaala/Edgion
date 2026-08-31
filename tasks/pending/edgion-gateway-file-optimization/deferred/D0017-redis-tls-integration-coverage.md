# D0017 — Add Successful Redis TLS Integration Coverage

## Meta
- Status: needs-implementation
- Severity: medium
- Discovered in: B0052, Redis integration coverage
- Category: integration-testing

## Evidence
Existing integration coverage proves plaintext/auth behavior and TLS rejection against a plaintext server, but not a successful Redis TLS handshake.

## Why deferred
Existing `edgion-tests/**` files are frozen during this campaign. A real TLS fixture and scenario require a dedicated integration-test change and review.

## Candidate next step
Add isolated coverage for scheme-only `rediss://`, default trust, custom CA, and mTLS, with bounded readiness polling and no modification during the current campaign.
