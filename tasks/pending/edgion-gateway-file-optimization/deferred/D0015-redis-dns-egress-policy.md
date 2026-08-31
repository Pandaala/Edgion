# D0015 — Enforce Redis DNS and Egress Policy

## Meta
- Status: needs-investigation
- Severity: high
- Discovered in: B0052, Redis Fred client construction
- Category: security

## Evidence
Operator-controlled Redis destinations use Fred's default resolver and dial path without canonical dial-time IP classification. Cluster and Sentinel can also discover addresses beyond configured seeds.

## Why deferred
Seed-only checks are insufficient; enforcement must cover Fred resolution, redirects, and discovered topology nodes without adding a second resolver mechanism.

## Candidate next step
Investigate Fred connector/resolver extension points and design one canonical EgressResolver/classify-at-dial integration for seeds and discovered addresses, or document a formally reviewed exception.
