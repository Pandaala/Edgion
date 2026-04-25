# BasicAuth Dynamic Test

Scenario: verify BasicAuth plugin rebuilds its internal user table when `secretRefs` list shrinks at runtime.

## Phases

| Phase | `secretRefs` | Alice | Bob |
|-------|--------------|-------|-----|
| initial | `[basic-auth-alice, basic-auth-bob]` | 200 | 200 |
| update  | `[basic-auth-alice]`                 | 200 | 401 |

Host: `basic-auth-dynamic.example.com`

Relies on existing secrets from `examples/test/conf/EdgionPlugins/BasicAuth/`:
- `Secret/edgion-default/basic-auth-alice` (01_Secret_default_basic-auth-users.yaml)
- `Secret/edgion-default/basic-auth-bob` (01b_Secret_default_basic-auth-bob.yaml)

Why removing the entire `secretRef` rather than changing the secret content:
BasicAuth uses a bcrypt LRU cache keyed by `secretRef` path. Modifying secret content risks
stale LRU hits masking bugs. Removing the whole ref makes the query path disappear entirely.
