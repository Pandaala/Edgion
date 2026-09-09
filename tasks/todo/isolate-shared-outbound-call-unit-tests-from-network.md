# Isolate Shared Outbound Call Unit Tests from Network

## Meta

| Key | Value |
|-----|-------|
| Created | 2026-09-09 |
| Status | pending |
| Type | testing-refactor |
| Priority | P2 |
| Issue | N/A |

## AI guide summary

Remove real loopback-network dependencies from the shared outbound-call library unit tests while preserving their retry, timeout, response-streaming, body-cap, framing, and failure-classification coverage.

## Problem

`edgion-gateway/src/plugins/http/common/outbound/call.rs` contains a large `#[cfg(test)]` matrix whose MockServer-backed cases open real loopback sockets from the library unit-test binary. That violates the project rule that unit tests must be deterministic, hermetic, and network-free. The behavior under test is important and cannot be deleted or weakened merely to satisfy the rule.

## Requirements

- Inventory every network-backed case in the outbound-call test module and map it to the precise behavior it protects.
- Introduce the smallest deterministic transport and clock seams needed to test request cloning/ownership, retry classification, bounded backoff, per-attempt timeout, `Retry-After`, response streaming, exact/overflow body caps, framing normalization, and cancellation without opening a socket.
- Keep production request execution on the canonical managed outbound client, TLS, egress, redirect-denial, and proxy-disabled path; test seams must not add a parallel production client or weaken final-dial SSRF checks.
- Preserve the existing public error and failure-policy behavior unless a separately reviewed design explicitly changes it.
- Remove the loopback-network dependency from library unit tests only after equivalent deterministic assertions exist.
- Do not modify, delete, rename, or depend on registration changes to existing `edgion-tests/**` files.

## Acceptance criteria

- The affected `edgion-gateway` library unit tests run with network access unavailable and retain equivalent behavioral coverage for every migrated case.
- No test-only convenience adds an owned copy, allocation, blocking lock, or slower shape to the production request path.
- Focused outbound-call and consumer tests pass, followed by workspace format/check/Clippy and all repository guards.
