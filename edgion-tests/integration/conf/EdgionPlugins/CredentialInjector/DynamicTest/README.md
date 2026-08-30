# CredentialInjector Dynamic Test

This scenario updates and deletes the Secret referenced by one CredentialInjector without changing the plugin resource.

| Phase | Expected behavior |
|-------|-------------------|
| `initial` | The upstream receives the initial bearer value |
| `updated` | The upstream receives the rotated bearer value |
| `deleted` | The request fails closed and does not reach the upstream |

The scenario restores the initial Secret after its assertions so later suites see a stable configuration.
