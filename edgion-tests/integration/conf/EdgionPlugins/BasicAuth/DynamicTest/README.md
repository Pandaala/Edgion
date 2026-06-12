# BasicAuth Dynamic Test

Scenario: verify BasicAuth plugin rebuilds its internal user table when `secretRefs` list shrinks at runtime.

## Phases

| Phase | `secretRefs` | Alice | Bob |
|-------|--------------|-------|-----|
| initial | `[basic-auth-alice, basic-auth-bob]` | 200 | 200 |
| update  | `[basic-auth-alice]`                 | 200 | 401 |

Host: `basic-auth-dynamic.example.com`

Relies on existing secrets from `edgion-tests/integration/conf/EdgionPlugins/BasicAuth/`:
- `Secret/edgion-default/basic-auth-alice` (01_Secret_default_basic-auth-users.yaml)
- `Secret/edgion-default/basic-auth-bob` (01b_Secret_default_basic-auth-bob.yaml)

Why removing the entire `secretRef` rather than changing the secret content:
Removing the whole ref exercises a structural reload (the plugin's user table must be
rebuilt), so a bug in the hot-reload path is much more likely to surface here than under
"same ref, password value changed". This pattern remains a useful template for other
plugins that may carry caches in the future, even though BasicAuth itself no longer keeps
a result cache.
