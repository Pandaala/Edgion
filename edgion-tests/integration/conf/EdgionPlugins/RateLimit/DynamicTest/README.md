# RateLimit Dynamic Test

Scenario: verify RateLimit plugin applies `onMissingKey` config change at runtime without state pollution.

## Phases

| Phase | `onMissingKey` | Request without `X-Rate-Key` |
|-------|----------------|-------------------------------|
| initial | `Allow` | 200 |
| update  | `Deny`  | 429 |

Host: `rate-limit-dynamic.example.com`

Why `onMissingKey` rather than counter exhaustion:
Rate counters are sliding-window with non-trivial timing effects, producing flaky tests.
`onMissingKey` is a pure logic-branch toggle — deterministic and cheap to assert.
