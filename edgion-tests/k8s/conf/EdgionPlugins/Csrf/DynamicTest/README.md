# CSRF Secret-rotation Dynamic Test

Standalone scenario that exercises CSRF signing-key generations through the shared Secret ref and requeue path.

| Phase | Assertion |
|-------|-----------|
| `initial` | Safe GET issues a token; POST with that token returns 200 |
| `updated` | A newly issued token returns 200; the pre-rotation token returns 401 |
| `deleted` | Unresolved signing key seals fail-closed with 500 |
| restore | Reapplying the initial Secret recovers issuance and validation |

The scenario polls each phase with `wait_phase`. It does not use a fixed sleep, and it restores the initial Secret even when a later assertion fails.
