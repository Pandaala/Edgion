# Audit Security-Protect EGC Hot Reload

## Meta

- Status: pending
- Type: refactor
- Priority: P1
- Issue: N/A

## AI guide summary

Inventory every Gateway consumer of `EdgionGatewayConfig.spec.securityProtect`, assign its correct immutable publication boundary, and correct policies that remain pinned to a listener-startup configuration after EGC updates. The work crosses request, connection, and TLS handshake lifecycles and must not force every setting into one unsuitable snapshot.

## Requirements

- Trace each security-protect field from configuration publication to every runtime consumer.
- Classify each field by listener, connection, handshake, or request lifetime before proposing changes.
- Make new generations visible at the appropriate boundary while preserving in-flight generation consistency.
- Preserve safe defaults, explicit operator escape hatches, binary-owned logging, redaction, access-log ownership, and hot-path bounds.
- Avoid a parallel configuration store or per-request parsing and blocking locks.

## Initial evidence

B0066 found listener-startup reads involving forwarded-header limits, global SNI matching, loopback-upstream policy across protocols, TLS proxy logging, and fallback SNI handling. Revalidate this inventory against the current tree before implementation.

## Out of scope

- The B0066 duplicate-Host request-policy correction.
- Rustls-only Gateway support.
- Unrelated security-policy redesign.
