# Add gRPC Timeout Upstream Rewrite Wire Coverage

## Meta

| Key | Value |
|-----|-------|
| Created | 2026-09-08 |
| Status | pending |
| Type | testing |
| Priority | P2 |
| Issue | N/A |

## AI guide summary

Add standalone cross-process coverage that observes the adjusted `grpc-timeout` value received by an upstream service, complementing the formatter's exact unit tests. Existing integration-file changes require explicit path-level authorization.

## Requirements

- Observe the actual `grpc-timeout` header received by the upstream after Gateway route matching, plugin execution, connection establishment, and attempt-local header rewriting.
- Cover a normal propagated deadline, elapsed-time adjustment, retry or repeated-attempt rewriting, and the zero-remaining `1n` race where a deterministic fixture can reach it.
- Assert valid gRPC timeout grammar, an eight-digit maximum numeric portion, and bounded deadline decay without relying on brittle exact wall-clock timing.
- Keep exact ceiling arithmetic, unit-transition behavior, representable-value non-shortening, and maximum-hour saturation in focused Gateway unit tests.
- Cover both pure gRPC and gRPC-Web only when the fixture can distinguish their upstream wire behavior without duplicating protocol infrastructure.
- Use canonical access-log diagnostics only; do not add request-path tracing, a parallel log, a new metric, or logging of request headers or bodies.
- Add a standalone scenario or obtain separate approval for the minimal shared fixture and registration changes. Do not modify existing integration-test files under this task's current authorization.

## Scope

### In scope

- A future upstream fixture that safely reports or records only the parsed timeout value needed by the assertion.
- Cross-process verification of the Gateway's existing upstream request rewrite boundary.
- Deterministic timing bounds and retry-attempt assertions.

### Out of scope

- Changing timeout parsing, formatting, deadline policy, retry policy, or Pingora lifecycle behavior.
- Logging raw request metadata or adding observability mechanisms.
- Modifying existing integration-test files without separate authorization.

## Acceptance criteria

- A real upstream observes the Gateway-adjusted `grpc-timeout` header rather than the original client value.
- The observed value uses a supported unit and no more than eight decimal digits.
- Timing assertions tolerate scheduling variance while proving that elapsed time is deducted and later attempts never regain budget.
- The deterministic zero-remaining case observes `1n`, or the task records concrete evidence that this race cannot be driven reliably and retains unit-level coverage for it.
- Existing gRPC deadline-exceeded behavior, gRPC-Web behavior, access-log ownership, and response streaming remain unchanged.
- Existing integration-test files remain unchanged unless a later authorization explicitly expands the scope.
