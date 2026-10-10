# CredentialInjector

> **🔌 Edgion extension**
>
> `CredentialInjector` is an `EdgionPlugins` request filter that reads an
> upstream bearer credential from a Kubernetes Secret or a Controller-managed
> `credentialSource` LinkSys and injects it only into the primary upstream
> request; it is not part of standard Gateway API.

## Quick start

Create an `Opaque` Secret whose `bearerToken` entry contains the raw token:

```yaml
apiVersion: v1
kind: Secret
metadata:
  name: provider-credential
stringData:
  bearerToken: provider-token-value
```

Reference it from a request-stage plugin. Omitting `target` selects bearer
injection and replaces the upstream `Authorization` header:

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: provider-credential
spec:
  requestPlugins:
    - type: CredentialInjector
      config:
        credentialRef:
          name: provider-credential
```

The upstream receives:

```http
Authorization: Bearer provider-token-value
```

The client cannot override the value: Edgion removes every inbound field with
the target name before scheduling the resolved credential.

## Custom header target

Use `type: Header` for providers that require a nonstandard header. The prefix
is static configuration and may be empty.

```yaml
requestPlugins:
  - type: CredentialInjector
    config:
      credentialRef:
        name: provider-credential
      target:
        type: Header
        name: X-Provider-Token
        valuePrefix: "Token "
```

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `credentialRef.name` | String | none | Required Secret or LinkSys name. |
| `credentialRef.namespace` | String | plugin namespace | Referenced object namespace. A cross-namespace reference requires a matching `ReferenceGrant`. |
| `credentialRef.group` | String | empty core API group | Omitted or empty selects the empty Kubernetes core API group; this group supports kind `Secret`. This Edgion field also accepts literal `core` for Secrets. A LinkSys credential source requires exact group `edgion.io` and kind `LinkSys`. |
| `credentialRef.kind` | String | `Secret` | `Secret` (core group) or explicit `LinkSys` (group `edgion.io`). Omitting kind still means `Secret`; group alone never infers `LinkSys`. |
| `target.type` | `Bearer` or `Header` | `Bearer` | Selects the upstream header shape. |
| `target.name` | String | none | Required only for `Header`. Header names used for routing, framing, forwarding, cookies, or hop-by-hop behavior are rejected. |
| `target.valuePrefix` | String | empty | Static prefix for `Header`, limited to 256 bytes. |

The complete injected value, including its prefix, is limited to 16 KiB. A
static Secret must be type `Opaque` (or have an empty type), contain
`bearerToken`, and hold a valid UTF-8 bearer token. Missing, denied, malformed,
deleted, or oversized credentials set `ResolvedRefs=False`; matching traffic
fails closed instead of reaching the upstream without authentication.

`target.name` and `target.valuePrefix` do not evaluate templates or DSL
expressions. Use plugin conditions to decide whether the injector runs, and use
ProxyRewrite or DSL for dynamic non-secret headers. Credential values never
enter the general template or expression context.

## Controller-managed OAuth2

`credentialRef` may point at a `LinkSys` of `type: credentialSource` instead of
a static Secret. The Controller acquires the OAuth2 client-credentials token
and embeds the **active** value on this plugin as `resolvedCredential` (Admin
`/configclient` redacts it). The Gateway never contacts the issuer.

One `EdgionPlugins` resource may contain at most one enabled LinkSys-backed
`CredentialInjector`. Static Secret-backed injector entries do not count toward
this limit.

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: provider-credential
spec:
  requestPlugins:
    - type: CredentialInjector
      config:
        credentialRef:
          group: edgion.io
          kind: LinkSys
          name: provider-oauth
```

### Secret publication (default)

Omitting `publication`, or setting `publication.persist: true`, writes a
generated Kubernetes Secret named after the consuming `EdgionPlugins` object.
This persist-on path is Kubernetes-only. On FS or etcd, the LinkSys is rejected
with `Accepted=False` and reason `UnsupportedConfigurationCenter`.

### In-memory publication

Set `publication.persist: false` on the LinkSys to keep generated tokens in a
Controller-local memory map instead of a Secret:

- no generated Secret is created or adopted
- allowed on a single-publisher Controller (leader-only Kubernetes, or FS/etcd)
- Kubernetes `ha_mode: all-serve` is rejected (`Accepted=False`, reason
  `UnsupportedHaMode`); the Controller does not call the issuer
- each LinkSys identity (`namespace/name/uid`) has one active token; successful
  publication replaces it without retaining a previous token for fallback
- the map is Controller-only: it is never a LinkSys spec, status, conf_sync, or
  Admin field. The Gateway still receives the active token through this
  plugin's `resolvedCredential`

```yaml
apiVersion: edgion.io/v1
kind: LinkSys
metadata:
  name: provider-oauth
spec:
  type: credentialSource
  config:
    provider:
      type: oauth2ClientCredentials
      tokenEndpoint: https://issuer.example.com/oauth2/token
      clientAuthentication:
        method: clientSecretBasic
        activeSecretRef:
          name: oauth-client
    publication:
      persist: false
```

## Credential lifecycle

The Controller proactively acquires the next token at the earlier of
`rotation.interval` (default `15m`) and half the issuer-provided lifetime,
with a small earlier-only jitter. Consumer validity reserves a 30-second safety
margin before issuer expiry. If acquisition fails, the current token remains
usable until its validity deadline while acquisition is retried. If Secret
publication fails after acquisition, the acquired token is retained for
publication retries without another issuer call.

`clientAuthentication.previousSecretRef` is a separate fallback for bootstrap
client credentials, used only when the issuer rejects the active client with
`invalid_client`. It does not select a previous generated access token.

Changing publication mode preserves credential material until replacement
publication completes:

- `true` to `false`: the Controller first places a live token in memory, then
  deletes generated Secrets that still match the expected owner and generation;
  failed deletions remain eligible for retry
- `false` to `true`: the Controller publishes a Secret for every current
  consumer and keeps the memory token until all current publications are
  consistent

Deleting an `EdgionPlugins` resource unregisters that consumer. Its managed
Secret is removed when other consumers remain; when the last consumer is gone,
the Controller also clears the LinkSys memory slot. Kubernetes owner references
provide a garbage-collection fallback for managed Secrets.

In-memory publication is intentionally ephemeral. Leadership loss, Controller
runtime reset or restart, LinkSys deletion or type change, LinkSys UID
replacement, and removal of the last consumer clear the slot. The Controller
reacquires the token when the source becomes active again. A Gateway may keep
using a generation already embedded in its PluginRuntime until `validUntil`; a
request fails closed when the Gateway has no unexpired resolved credential.

## Ordering and isolation

Credential mutation follows normal request-plugin declaration order:

- a later `CredentialInjector` for the same header wins;
- a later header set, append, or remove cancels the pending injection;
- a mirror declared before the injector may observe the client's original
  header, while a mirror declared after it observes no target header;
- mirrors never receive the resolved Secret value;
- retries for one logical request keep the credential generation first
  resolved by that request, including backend-level `ExtensionRef` plugins.

The resolved credential is materialized only in the final attempt-local primary
upstream header block. It is not written back into the downstream request and
is redacted from configuration inspection, debug output, and plugin logs.

## Cookie status

Structured Cookie injection is intentionally not supported. Cookie fields have
merge, duplicate-name, ordering, and quoting semantics that cannot safely be
represented as a generic header overwrite. Use bearer or a dedicated custom
header until Edgion provides a separate structured Cookie model.

## Related documents

- [Plugin Composition and Execution Order](../plugin-composition.md)
- [RequestMirror](./request-mirror.md)
