# CredentialInjector

> **Edgion extension**
>
> `CredentialInjector` reads an upstream bearer credential from a Kubernetes
> Secret and injects it only into the primary upstream request.

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
| `credentialRef.name` | String | none | Required Secret name. |
| `credentialRef.namespace` | String | plugin namespace | Secret namespace. A cross-namespace reference requires a matching `ReferenceGrant`. |
| `credentialRef.group` | String | core API group | If set, must identify the Kubernetes core API group. |
| `credentialRef.kind` | String | `Secret` | If set, must be `Secret`. |
| `target.type` | `Bearer` or `Header` | `Bearer` | Selects the upstream header shape. |
| `target.name` | String | none | Required only for `Header`. Header names used for routing, framing, forwarding, cookies, or hop-by-hop behavior are rejected. |
| `target.valuePrefix` | String | empty | Static prefix for `Header`, limited to 256 bytes. |

The complete injected value, including its prefix, is limited to 16 KiB. The
Secret must be type `Opaque` (or have an empty type), contain `bearerToken`, and
hold a valid UTF-8 bearer token. Missing, denied, malformed, deleted, or
oversized credentials set `ResolvedRefs=False`; matching traffic fails closed
instead of reaching the upstream without authentication.

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
