# EdgionBackend SingleProvider Dynamic Test

Scenario: `AiBackendSecretRotation` — verify an EdgionBackend provider credential
can be rotated at runtime without editing the EdgionBackend or restarting the
gateway/controller.

## Phases

| Phase | `Secret/edgion-test/ai-provider-credential` `api-key` | Upstream-observed `credential_generation` |
|-------|--------------------------------------------------------|-------------------------------------------|
| initial | `ai-provider-test-key`         | `a` |
| updated | `ai-provider-rotated-test-key` | `b` |

Host: `ai-default.example.com` (shares the static `EdgionBackend/SingleProvider` fixtures)

The scenario explicitly applies `initial/00_Secret_ai-provider.yaml`, then
`updates/00_Secret_ai-provider.yaml`, and finally restores the initial Secret.
The explicit reset keeps repeated dynamic sweeps independent even though the
parent `SingleProvider` fixture starts with the same initial credential.

The `updated` phase additionally asserts the gateway Admin `/configclient/edgionbackend/list`
output still renders every credential as `"[redacted]"`, so a rotation can never start
leaking the secret material through the admin surface.

Why rotate the Secret rather than repoint `secretRef`:
repointing the reference changes the EdgionBackend spec and explicitly publishes an
updated backend resource. Changing only the Secret payload isolates Secret dependency
re-resolution, so a stale-credential cache in the gateway shows up as a
`credential_generation: "a"` observation instead of being masked by an explicit backend
spec update.
