# RateLimit Integration Tests

## Overview

The RateLimit plugin uses Pingora's Count-Min Sketch (CMS) algorithm to provide high-performance rate limiting.

## Test Configuration

### Limit Settings

- **Rate:** 5 requests
- **Interval:** 10-second window
- **Key Source:** Header (`X-Rate-Key`)
- **Reject Status:** 429

### Test Route

- **Host:** `rate-limit.example.com`
- **Path:** `/test/rate-limit/*`
- **Gateway Port:** `31180`

## Test Scenarios

| Test Name | Description | Expected Result |
|-----------|-------------|-----------------|
| `allows_within_limit` | Requests remain within the limit | `200 OK` |
| `blocks_over_limit` | Requests exceed the limit | `429 Too Many Requests` |
| `headers_present` | Rate-limit headers are returned | `X-RateLimit-*` headers |
| `different_keys_independent` | Different keys are limited independently | Separate counters |

## Running the Tests

```bash
# Run the RateLimit suite
cargo run --bin test-client -- --suite rate-limit
```

## Notes

1. Each test uses a unique `X-Rate-Key` to avoid cross-test interference.
2. The 10-second window helps keep the suite from timing out.
3. CMS may introduce small counting errors, which is expected behavior.
