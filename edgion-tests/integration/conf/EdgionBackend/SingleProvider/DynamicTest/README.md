# EdgionBackend SingleProvider Dynamic Test

Scenario: `AiBackendSecretRotation` — verify an EdgionBackend provider credential
can be rotated at runtime, with no gateway/controller restart and no config-server
generation change.

## Phases

| Phase | `Secret/edgion-test/ai-provider-credential` `api-key` | Upstream-observed `credential_generation` |
|-------|--------------------------------------------------------|-------------------------------------------|
| initial | `ai-provider-test-key`         | `a` |
| updated | `ai-provider-rotated-test-key` | `b` |

Host: `ai-default.example.com` (shares the static `EdgionBackend/SingleProvider` fixtures)

There is no `initial/` directory: the initial state is the statically loaded
`03_Secret_ai-provider.yaml` from the parent directory. The scenario only applies
`updates/Secret_ai-provider.yaml`.

The `updated` phase additionally asserts the gateway Admin `/configclient/edgionbackend/list`
output still renders every credential as `"[redacted]"`, so a rotation can never start
leaking the secret material through the admin surface.

Why rotate the Secret rather than repoint `secretRef`:
repointing the ref is an EdgionBackend spec change, which is exactly the case that
*would* legitimately bump the config-server generation. Changing only the Secret payload
isolates the credential-resolution path, so a stale-credential cache in the gateway shows
up as a `credential_generation: "a"` observation instead of being masked by a full resource
re-push.
