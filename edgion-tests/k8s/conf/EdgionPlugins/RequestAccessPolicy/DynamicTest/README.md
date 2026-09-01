# RequestAccessPolicy HMAC Dynamic Test

This scenario rotates the contents of one referenced HMAC credential Secret without changing the RequestAccessPolicy resource.

| Phase | Initial key | Rotated key |
|-------|-------------|-------------|
| `initial` | 200 | 403 |
| `updated` | 403 | 200 |

The scenario explicitly reapplies and waits for the initial phase, updates the Secret, waits for the business behavior to switch, and restores the initial Secret even when the rotation assertion fails.
