# Form/JSON Transform Plugin

> **🔌 Edgion extension**
>
> `FormJsonTransform` is a request plugin provided by the `EdgionPlugins` CRD.

## Overview

`FormJsonTransform` converts a bounded request body between
`application/x-www-form-urlencoded` and JSON. It is intended for flat legacy
form integrations. Direction is explicit and is never inferred from a client
header.

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: form-to-json
spec:
  requestPlugins:
    - type: FormJsonTransform
      config:
        direction: formToJson
```

| Field | Required | Values |
|-------|----------|--------|
| `config.direction` | Yes | `formToJson`, `jsonToForm` |
| `body.maxBodySize` | No | Defaults to `64KiB`; maximum `1MiB`. The same effective limit applies to input and output |

The plugin is request-only and always fail-closed. `body.conditions` and
`body.onReadFailure: failOpen` are rejected. Entry-level `conditions` may
still skip the entire plugin before body capture.

## Mapping

For form-to-JSON:

- percent escapes must be complete hexadecimal byte escapes and decoded bytes
  must be valid UTF-8;
- `+` decodes to space;
- one key/value pair becomes a JSON string member;
- repeated keys become an array of strings in occurrence order;
- no number, boolean, null, bracket, or nesting inference occurs;
- empty segments are ignored, while empty keys and values are retained;
- an empty body becomes `{}`.

For JSON-to-form, the root must be an object. Member values may be strings,
numbers, booleans, or non-empty arrays of those scalar types. Null, objects,
nested arrays, and empty arrays are rejected. An empty object becomes an empty
body.

Keys are emitted in lexical order. Array value order is retained. Form output
uses the WHATWG form encoding set: ASCII alphanumeric bytes plus `*`, `-`, `.`,
and `_` remain literal, space becomes `+`, and every other UTF-8 byte is
uppercase `%HH`. JSON numbers use the gateway JSON parser's canonical
representation; their original lexical spelling is not retained.

This mapping is not a byte-level round trip. Cross-key form tuple order cannot
be represented in a JSON object, form input is string-typed, and a single value
becomes a scalar rather than a one-element array.

## Limits and Request Requirements

Each conversion accepts at most 256 pairs, a 256-byte decoded key, and a
16-KiB decoded scalar value. JSON also uses the shared depth and node limits.
Limits are enforced before publishing a replacement body.

`formToJson` requires exactly one
`Content-Type: application/x-www-form-urlencoded`; its only optional parameter
is `charset=utf-8`. `jsonToForm` accepts `application/json` or a concrete
`application/*+json` type with UTF-8. Both directions require absent
`Content-Encoding` or one `identity` value.

After conversion, `Content-Type` becomes exactly `application/json` or
`application/x-www-form-urlencoded`. Upstream framing is recalculated from the
new body. An accepted `Content-Encoding: identity` is retained.

| Condition | HTTP status | Response body |
|-----------|-------------|---------------|
| Successful conversion | Continue to backend | N/A |
| Malformed encoding/JSON or unsupported JSON shape | `400` | `Invalid request body` |
| Unsupported media type, charset, or content encoding | `415` | `Unsupported media type` |
| Body, output, pair, key, or value limit exceeded | `413` | Empty |
| Body storage or mutation failure | `500` | Empty |

## Composition

Plugins observe the body and media type published by earlier plugins. A
`formToJson` entry may therefore precede `JsonSchemaValidation` or the JSON
body operations in `ProxyRewrite`. A later transform applies its own body
limit to the already transformed body. Two transforms in the same direction
cause the second entry to reject the changed media type with `415`.

The request body uses the shared request-scoped body store, including
spill-to-disk and retry replay. No response body is buffered.
