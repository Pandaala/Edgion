# AI time-boundary tests use controlled clocks

## Decision

Tests for AI quota windows, final reconciliation deadlines, outlier recovery,
and adaptive telemetry must not rely on a short unobserved interval of wall
clock time. Unit tests pass explicit epoch values or use Tokio's paused clock.
Integration tests that must exercise real process clocks reserve enough time
before a boundary for every operation performed inside the asserted window.

`build_targets` receives an explicit epoch millisecond value. Production passes
the current system time, while unit tests use fixed values to prove that quota
buckets rotate at the exact window boundary and that `retry_after` resets from
one millisecond to the full window.

The credential-exhaustion integration case starts its adjacent request pair
only when at least 25 seconds remain in the minute bucket. Each of its two HTTP
requests has a 10-second timeout, and all access-log and Redis inspection
happens after both requests, so a successful pair cannot cross the bucket
boundary. The half-open fixture uses a 10-second ejection period, longer than
its access-log polling budget, and waits for that configured period before
exercising concurrent probe admission. Credential exhaustion uses a 90-second
harness timeout, while the half-open case uses 120 seconds to cover its
approximately 100-second worst-case sequence of bounded requests and polling.
Unrelated cases keep the 30-second default.

## Regression coverage

- Final quota reconciliation tests use `start_paused = true` and assert the
  exact retry sequence.
- Adaptive settlement reads the epoch second after all finalization work before
  ticking beyond the completed buckets.
- Quota target tests use fixed epoch milliseconds, including 59,999 ms and
  60,000 ms on a one-minute window.
- The integration cases retain their cross-process assertions without racing a
  one-second or near-expiry minute boundary.

TPM denial and post-window credential recovery are separate coverage topics;
they are not implied by stabilizing these existing tests.
