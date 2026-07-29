# JSON Schema Validation Plugin

> **🔌 Edgion extension**
>
> `JsonSchemaValidation` is a request plugin provided by the `EdgionPlugins` CRD. It is not part of the standard Gateway API.

## Overview

`JsonSchemaValidation` validates an incoming JSON request body against an inline JSON Schema before the request reaches the backend. It uses JSON Schema Draft 2020-12, rejects malformed JSON and duplicate object keys, and forwards valid bodies byte-for-byte without re-encoding them.

The plugin is request-only and fail-closed. Each `requestPlugins` list may contain at most one `JsonSchemaValidation` entry. Compose multiple constraints inside one schema with keywords such as `allOf`.

## Quick Start

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: validate-create-user
spec:
  requestPlugins:
    - type: JsonSchemaValidation
      config:
        schema:
          $schema: https://json-schema.org/draft/2020-12/schema
          type: object
          required: [name, email]
          properties:
            name:
              type: string
              minLength: 1
            email:
              type: string
              format: email
          additionalProperties: false
```

Reference this resource from an `HTTPRoute` with an `ExtensionRef` filter. See [Plugin Composition and Execution Order](../plugin-composition.md).

## Configuration

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `config.schema` | JSON value | Yes | Inline Draft 2020-12 schema |
| `body.maxBodySize` | Byte size | No | Per-entry body limit. Defaults to `64KiB`; the maximum accepted value is `1MiB` |
| `conditions` | Common plugin conditions | No | Skips the complete plugin, including body capture, when conditions do not match |

`body.conditions` is not supported because body validation must not become conditionally fail-open. Entry-level `alias` and `dye` use the common plugin entry contract.

The inline schema has fixed safety limits, including 64 KiB serialized size, depth 64, 4,096 schema nodes, 64 patterns, and bounded regular-expression compilation. A configuration exceeding a limit is rejected.

## Supported Schema Features

The dialect is Draft 2020-12 with assertions for safe supported built-in
`format` values. Unknown formats remain annotations. `format: regex` is
rejected because it would compile request-controlled regular expressions on
the request path.

Only local JSON Pointer references into the root document are supported:

```yaml
config:
  schema:
    type: object
    properties:
      address:
        $ref: "#/$defs/address"
    $defs:
      address:
        type: object
        required: [city]
        properties:
          city:
            type: string
```

External references and retrieval are never allowed. `$id`, `$anchor`, `$dynamicAnchor`, `$dynamicRef`, `$recursiveRef`, `unevaluatedProperties`, and local reference cycles are rejected. Use schemas that are fully self-contained and acyclic.

## Request Requirements

For a non-empty body, the plugin accepts:

- `Content-Type: application/json`;
- an `application/*+json` subtype such as `application/problem+json`;
- no `charset`, or `charset=utf-8` with case-insensitive matching;
- other syntactically valid media-type parameters;
- an absent `Content-Encoding`, or `Content-Encoding: identity`.

Missing, malformed, duplicate, or comma-combined `Content-Type` values are rejected. Non-JSON media types, non-UTF-8 charsets, and encoded bodies such as gzip are also rejected.

An empty body is always treated as invalid JSON and returns `400`, even when `Content-Type` is absent.

## Outcomes

| Condition | HTTP status | Response body |
|-----------|-------------|---------------|
| Valid JSON matching the schema | Continue to backend | N/A |
| Empty, malformed, duplicate-key, over-work-budget, or schema-mismatching JSON | `400` | `Invalid request body` |
| Unsupported media type, charset, or content encoding | `415` | `Unsupported media type` |
| Logical or physical body limit exceeded | `413` | Empty |
| Body read timeout | `408` | Empty |
| Body capture, storage, validator, or unavailable-runtime failure | `500` | Empty |

Validation details, instance values, schema content, storage errors, and panic details are never exposed to clients or plugin logs.

## Examples

### Restrict an event envelope

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: validate-event
spec:
  requestPlugins:
    - type: JsonSchemaValidation
      body:
        maxBodySize: 256KiB
      config:
        schema:
          type: object
          required: [eventType, payload]
          properties:
            eventType:
              enum: [created, updated]
            payload:
              type: object
          additionalProperties: false
```

### Validate an array with a bounded size

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: validate-batch
spec:
  requestPlugins:
    - type: JsonSchemaValidation
      config:
        schema:
          type: array
          minItems: 1
          maxItems: 100
          items:
            type: object
            required: [id]
            properties:
              id:
                type: string
            additionalProperties: false
```

### Require one of two shapes

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: validate-command
spec:
  requestPlugins:
    - type: JsonSchemaValidation
      config:
        schema:
          oneOf:
            - type: object
              required: [start]
              properties:
                start:
                  type: boolean
              additionalProperties: false
            - type: object
              required: [stop]
              properties:
                stop:
                  type: boolean
              additionalProperties: false
```

## Operational Behavior

Schema compilation occurs during configuration validation and runtime publication, never on the request path. Invalid initial configuration reports `Accepted=False`; traffic is sealed with a fixed `500` response. An invalid hot reload also replaces the previous runtime with the fail-closed runtime rather than retaining last-known-good behavior. Applying a later valid schema restores normal traffic.

The request body uses the shared request-scoped body store. A body may spill to disk according to the gateway body policy, but accepted bytes are replayed to the backend unchanged.

## Security and Performance

- Keep `additionalProperties: false` where unknown input fields should be rejected.
- Set `body.maxBodySize` to the smallest limit the API needs; the default is `64KiB`.
- Do not place secrets or internal implementation details in schema annotations.
- Prefer simple, anchored patterns. Unsupported backtracking-only regular-expression features are rejected.
- The plugin builds a JSON value for each accepted-size request. Avoid enabling it on routes that do not require structural body validation.

## FAQ

### Can the plugin load a schema from a URL or file?

No. Schemas must be inline, and external retrieval is disabled.

### Does it modify valid JSON?

No. Validation uses a parsed representation, but the original stored bytes are forwarded.

### Can I configure a custom error response?

No. The first release deliberately uses fixed status codes and generic bodies to avoid leaking validation details.

### Can I validate compressed JSON?

No. Send identity-encoded JSON. Any other `Content-Encoding` returns `415`.
