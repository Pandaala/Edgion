# H2-013: Align Fork H2 Documentation with Verified Evidence

## Severity

P2 maintenance and security-review risk.

## Evidence

Fork documentation states that rejected trailers, GOAWAY exclusion, and local
reset cannot be laundered, and that unit tests independently prove ordering.
Current task tests instead record zero-DATA/direct trailer laundering and leave
GOAWAY, EOF/read-error poison, and several lifecycle contracts ignored.

## Required change

- Mark every unresolved contract as conditional or unsupported in current
  production behavior.
- Do not describe ordering as independently proved until the raw fixture has a
  client-observable processing barrier.
- Update documentation only after enabled tests demonstrate the stated claim
  at minimum and current h2 resolutions.

## Acceptance tests

- Every behavior claim maps to an enabled, reproducible test.
- Documentation review confirms no ignored contract is described as enforced.

## Related files

- `pingora/docs/edgion-fork/h2-end-stream.md`
- `01-design.md`
- `06-decisions.md`
