# RateLimit Dynamic Test

Scenario: verify RateLimit plugin applies `onKeyMiss` config change at runtime without state pollution.

## Phases

| Phase | `onKeyMiss` | Request without `X-Rate-Key` |
|-------|----------------|-------------------------------|
| initial | `Allow` | 200 |
| update  | `Deny`  | 429 |

Host: `rate-limit-dynamic.example.com`

Why `onKeyMiss` rather than counter exhaustion:
Rate counters are sliding-window with non-trivial timing effects, producing flaky tests.
`onKeyMiss` is a pure logic-branch toggle — deterministic and cheap to assert.
