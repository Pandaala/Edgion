# Align Route Retry Limits with the Pingora Loop Ceiling

## Status

Ready for a separate design and implementation task.

## Problem

Ordinary HTTP/GRPC route retry configuration and
`EdgionGatewayConfig.spec.maxRetries` can exceed the locked Pingora proxy loop's
effective 16-iteration ceiling. Edgion can therefore accept and report a retry
count that the data plane cannot execute.

## Constraints

- Define one explicit retry-count contract across resource/schema validation,
  Gateway config resolution, route policy, listener/server construction, and
  user documentation.
- Account for whether the configured value means retries or total attempts; do
  not introduce an off-by-one clamp.
- Preserve the separately bounded AI retry policy unless the unified contract
  explicitly proves it should change.
- Reject or resolve unsupported values before the request hot path; do not parse
  or clamp them per request.
- Do not patch the locked Pingora loop without an explicit fork/API design and
  corresponding regression coverage.

## Acceptance direction

Add boundary tests at the maximum supported value and one value above it, prove
the configured retry count matches executed attempts for HTTP and GRPC routes,
and update schema/config/user guidance consistently.
