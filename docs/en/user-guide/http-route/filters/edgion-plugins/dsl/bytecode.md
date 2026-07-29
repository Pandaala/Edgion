# Bytecode Mode

## What Is Bytecode Mode

The DSL plugin supports two loading modes: `source` (source code) and `bytecode` (pre-compiled bytecode).

**Source mode**: When the Controller receives a configuration, it compiles the source code into bytecode on-the-fly and pushes it to the Gateway via gRPC. Compilation is re-triggered on every configuration change or Controller restart.

**Bytecode mode**: Pre-compiled bytecode is provided directly in the configuration. The Controller skips the compilation step and pushes the bytecode to the Gateway immediately.

When both `source` and `bytecode` are present in the configuration, `bytecode` takes precedence and the `source` field is ignored.

## Bytecode Format

Bytecode is transmitted as a Base64-encoded JSON string.

### JSON Structure

```json
{
  "version": 2,
  "code": [...],
  "constants": [...],
  "local_count": 16,
  "max_loop_depth": 2,
  "source": "..."
}
```

Field descriptions:

| Field | Type | Description |
|-------|------|-------------|
| `version` | integer | Bytecode format version. Current version is `2`. |
| `code` | array | Instruction sequence (`Vec<OpCode>`), executed by the VM one instruction at a time. |
| `constants` | array | Constant pool (`Vec<Constant>`), referenced by instructions via index. |
| `local_count` | integer | Number of local variable slots. |
| `max_loop_depth` | integer | Maximum loop nesting depth. |
| `source` | string | Optional. The original source code, used only for debugging and log output. |

### Constant Types

Each entry in the constant pool is one of the following types:

- `Str`: a string constant
- `Int`: an integer constant
- `Regex`: a regular expression used by string-only find/replace operations
- `StreamingRegex`: a forward-DFA pattern used by incremental `.matches()` operations
- `JsonPath`: a validated definite JSONPath used by `.json_get()`

## When to Use Bytecode

| Scenario | Recommended Mode | Reason |
|----------|-----------------|--------|
| Development and debugging | `source` | Readable, easy to modify, no pre-compilation step required. |
| High-traffic routes | `bytecode` | Eliminates compilation overhead; Gateway starts or reloads faster. |
| CI/CD pipelines | `bytecode` | Compilation happens at build time; bytecode is used directly at deploy time, ensuring consistency. |
| Cross-version distribution | `source` or rebuilt `bytecode` | Bytecode versions are intentionally strict; rebuild it for the target Edgion release. |

## Size Limits

- Maximum decoded size: **256 KB** (constant `MAX_DECODED_BYTECODE_LEN`)
- Maximum Base64-encoded size: approximately **341 KB** (Base64 expansion factor ~4/3)
- If the limit is exceeded, the Controller rejects the configuration and returns a validation error.

## Version Compatibility

Bytecode carries a version number in the `version` field, which is strictly validated during deserialization.

- If the version does not match, the Controller refuses to load the bytecode and reports a version conflict in the logs.
- If the bytecode format version changes after an Edgion upgrade, existing bytecode must be recompiled.
- It is recommended to bind bytecode to a specific Edgion version in your CI/CD pipeline to avoid version drift.

Current version: `BYTECODE_VERSION = 2`

Version 2 is a breaking format change: `req.body` is now an opaque `Body` value and body operations use dedicated instructions and constants. Version 0, version 1, and unversioned payloads are rejected; recompile them from source with the target Edgion version.

## Security Validation

The following static checks are performed during deserialization. If any check fails, the bytecode is rejected:

1. **Version match**: The `version` field matches the version expected by the runtime.
2. **Constant index bounds**: All constant indices referenced by `LoadConst` instructions fall within the bounds of the constant pool.
3. **Instruction/constant type match**: Value loads accept only `Str`/`Int`; `.matches()`, string regex operations, and `.json_get()` require `StreamingRegex`, `Regex`, and `JsonPath` constants respectively.
4. **Regex reconstruction**: Regex constants are recompiled with bounded automaton size to verify syntax and resource limits in the current runtime.
5. **JSONPath validation**: Every `JsonPath` constant is parsed again and must remain a definite path.

## Configuration Examples

### Using Source Code (Development)

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: auth-check
spec:
  requestPlugins:
      type: Dsl
      config:
        name: "auth-check"
        source: |
          let token = req.header("Authorization")
          if token == nil {
            return deny(401, "Unauthorized")
          }
```

### Using Bytecode (Production)

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: auth-check
spec:
  requestPlugins:
      type: Dsl
      config:
        name: "auth-check"
        bytecode: "BASE64_ENCODED_BYTECODE_V2"
```

### Providing Both Source and Bytecode

When both `source` and `bytecode` are configured, `bytecode` takes effect. The `source` field is retained only as a debugging reference — it may be embedded in the bytecode's own `source` field if it was included at compile time.

```yaml
config:
  name: "auth-check"
  source: |
    let token = req.header("Authorization")
    if token == nil {
      return deny(401, "Unauthorized")
    }
  bytecode: "BASE64_ENCODED_BYTECODE_V2"
```

## Related Documentation

- [DSL Overview](./README.md)
- [Sandbox Security Model](./sandbox-security.md)
- [Language Syntax Reference](./language-syntax.md)
