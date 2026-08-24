# Webhook KeyGet

> **🔌 Edgion extension**
>
> WebhookKeyGet resolves values from a LinkSys webhook into request context, once per request. It is provided by the `EdgionPlugins` CRD and is not part of standard Gateway API.

## What is WebhookKeyGet?

WebhookKeyGet calls a LinkSys resource with `spec.type: webhook`, extracts one field from the response, and writes that field into a named context variable. Later plugins on the same request read the variable with `from: { type: ctx, name: … }` — they never call the webhook themselves.

Use this plugin when you need a **value** (a key, a tenant id, an enrichment field). Use [Forward Auth](./forward-auth.md) when you need an **allow/deny verdict**.

## Quick Start

### Simplest configuration

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: tenant-from-webhook
  namespace: production
spec:
  requestPlugins:
    - type: WebhookKeyGet
      config:
        vars:
          - name: tenant_id
            webhookRef: "production/tenant-resolver"
            extract:
              source: body
              path: "data.tenant_id"
            default: "public"
```

The referenced LinkSys (`production/tenant-resolver`) owns the outbound URL, method, headers, and timeout. WebhookKeyGet only names the variable, the webhook, the extract, and an optional fallback.

### Default-value notes

- `vars` is required and must contain at least one entry.
- Resolution runs **once per request**, in list order. A later plugin that reads the same ctx name sees the already-written value.
- A failed or empty resolution writes `default` when set; with no `default` the variable is left unset.
- Failures never reject the request. Downstream plugins decide what an unset or fallback value means.

---

## Configuration parameters

### Basic parameters

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `vars` | Array\<Object\> | **Required** | List of webhook variables to resolve. Must be non-empty |

### `vars[]` fields

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `name` | String | **Required** | Target context variable name |
| `webhookRef` | String | **Required** | LinkSys webhook reference as `namespace/name` |
| `extract` | Object | **Required** | How to take the value from the webhook response; see the extract table |
| `pathOverride` | String | none | Per-call path. Honored only when the webhook sets `request.path.allowOverride: true` |
| `default` | String | none | Value written when resolution yields nothing |

### `extract` fields

| `source` | Extra field | Reads from |
|----------|-------------|------------|
| `header` | `name` | A response header (case-insensitive) |
| `cookie` | `name` | A `Set-Cookie` name |
| `body` | `path` | A JSON dot-path (`data.user_id`, `result.0.key`) |
| `bodyText` | — | The entire response body, trimmed |

---

## Common configuration scenarios

### 1. Resolve a tenant id from the webhook body

```yaml
- type: WebhookKeyGet
  config:
    vars:
      - name: tenant_id
        webhookRef: "production/tenant-resolver"
        extract:
          source: body
          path: "data.tenant_id"
        default: "public"
```

### 2. CtxSet first, then WebhookKeyGet

Webhook templates can read `${ctx:NAME}`. Those names must already exist, so CtxSet runs **before** WebhookKeyGet.

```yaml
requestPlugins:
  - type: CtxSet
    config:
      vars:
        - name: action
          value: "resolve"
        - name: user_id
          from:
            type: header
            name: X-User-Id
          default: "anon"
  - type: WebhookKeyGet
    config:
      vars:
        - name: resolved_user
          webhookRef: "production/user-resolver"
          extract:
            source: body
            path: "data.user"
          default: "fallback"
```

A single CtxSet entry must not mix local vars and a webhook var. Split them: CtxSet keeps the local vars; WebhookKeyGet follows.

### 3. WebhookKeyGet then KeyAuth (ordering requirement)

KeyAuth can authenticate a ctx variable. WebhookKeyGet must run **first** so the variable exists.

```yaml
requestPlugins:
  - type: WebhookKeyGet
    config:
      vars:
        - name: api_key
          webhookRef: "production/key-resolver"
          extract:
            source: body
            path: "data.key"
  - type: KeyAuth
    config:
      keySources:
        - type: ctx
          name: api_key
      secretGroups:
        - name: default
          secretRefs:
            - name: api-keys
              namespace: production
```

**A reversed list is a silent auth bypass.** If KeyAuth runs first, `api_key` is empty. KeyAuth then sees no key (401) — or, if it also has a header/query source, it may authenticate a client-supplied value and never wait for the webhook. There is no cross-plugin ordering check (W-1 declined one). Put WebhookKeyGet above every plugin that reads the variable.

---

## FAQ

### Q1: Why did the context variable stay empty?

**A**: The webhook call failed or the extract path missed, and the var has no `default`. Check the LinkSys `Accepted` condition, the extract path, and whether a `presence: required` origin field was missing from the inbound request.

### Q2: Why was `pathOverride` ignored?

**A**: The webhook resource did not set `request.path.allowOverride: true`. Without that flag the template path (or the frozen incoming path) is used.

### Q3: When should I use Forward Auth instead?

**A**: Use Forward Auth when the external service decides allow/deny. Use WebhookKeyGet when the external service returns a **value** that later plugins consume.

## Get help

If you run into problems:

1. Check the FAQ section of this document
2. Check the gateway logs
3. Open an issue on GitHub

---

## Security best practices

### ✅ Recommended

- Keep WebhookKeyGet **before** every consumer of the ctx variable (KeyAuth, RateLimit, DSL).
- Point KeyAuth `keySources` at `{ type: ctx, name: … }` only when the webhook is the intended source — do not also list a client-controlled header unless that is deliberate.
- Prefer `default` only for non-security enrichment. Do not default an auth key to a valid credential.

### ❌ To avoid

- Putting KeyAuth (or any auth plugin) before WebhookKeyGet and expecting the webhook value to be required.
- Treating an unset ctx var as authenticated.
- Duplicating the webhook call in KeyAuth or CtxSet. The accessor `type: webhook` is not the operator-facing consumer.

---

## Complete configuration example

```yaml
apiVersion: edgion.io/v1
kind: LinkSys
metadata:
  name: key-resolver
  namespace: production
spec:
  type: webhook
  config:
    target:
      kind: Service
      name: key-service
      namespace: production
      port: 8080
    request:
      path:
        template: "/resolve"
      method:
        template: "GET"
      headers:
        origin:
          - name: X-Tenant-Id
    timeoutMs: 3000
    success:
      statusCodes: [200]
---
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: webhook-then-key-auth
  namespace: production
spec:
  requestPlugins:
    - type: WebhookKeyGet
      config:
        vars:
          - name: api_key
            webhookRef: "production/key-resolver"
            extract:
              source: body
              path: "data.key"
    - type: KeyAuth
      config:
        keySources:
          - type: ctx
            name: api_key
        secretGroups:
          - name: default
            secretRefs:
              - name: api-keys
                namespace: production
---
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: protected
  namespace: production
spec:
  parentRefs:
    - name: edgion
  hostnames:
    - "api.example.com"
  rules:
    - filters:
        - type: ExtensionRef
          extensionRef:
            group: edgion.io
            kind: EdgionPlugins
            name: webhook-then-key-auth
      backendRefs:
        - name: api-service
          port: 8080
```

---

## Related links

- [LinkSys webhook connector](../../../../ops-guide/infrastructure/connectors.md)
- [Key Auth](./key-auth.md)
- [Ctx Setter](./ctx-setter.md)
- [Forward Auth](./forward-auth.md)
