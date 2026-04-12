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
  "version": 1,
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
| `version` | integer | Bytecode format version. Current version is `1`. |
| `code` | array | Instruction sequence (`Vec<OpCode>`), executed by the VM one instruction at a time. |
| `constants` | array | Constant pool (`Vec<Constant>`), referenced by instructions via index. |
| `local_count` | integer | Number of local variable slots. |
| `max_loop_depth` | integer | Maximum loop nesting depth. |
| `source` | string | Optional. The original source code, used only for debugging and log output. |

### Constant Types

Each entry in the constant pool is one of the following types:

- `Str`: a string constant
- `Int`: an integer constant
- `Regex`: a pre-compiled regular expression (compiled at bytecode serialization time)

## When to Use Bytecode

| Scenario | Recommended Mode | Reason |
|----------|-----------------|--------|
| Development and debugging | `source` | Readable, easy to modify, no pre-compilation step required. |
| High-traffic routes | `bytecode` | Eliminates compilation overhead; Gateway starts or reloads faster. |
| CI/CD pipelines | `bytecode` | Compilation happens at build time; bytecode is used directly at deploy time, ensuring consistency. |
| Cross-version distribution | `bytecode` | Bytecode carries a version number and can be safely transferred and stored long-term. |

## Size Limits

- Maximum decoded size: **256 KB** (constant `MAX_DECODED_BYTECODE_LEN`)
- Maximum Base64-encoded size: approximately **341 KB** (Base64 expansion factor ~4/3)
- If the limit is exceeded, the Controller rejects the configuration and returns a validation error.

## Version Compatibility

Bytecode carries a version number in the `version` field, which is strictly validated during deserialization.

- If the version does not match, the Controller refuses to load the bytecode and reports a version conflict in the logs.
- If the bytecode format version changes after an Edgion upgrade, existing bytecode must be recompiled.
- It is recommended to bind bytecode to a specific Edgion version in your CI/CD pipeline to avoid version drift.

Current version: `BYTECODE_VERSION = 1`

## Security Validation

The following static checks are performed during deserialization. If any check fails, the bytecode is rejected:

1. **Version match**: The `version` field matches the version expected by the runtime.
2. **Constant index bounds**: All constant indices referenced by `LoadConst` instructions fall within the bounds of the constant pool.
3. **Regex constant type**: All constants referenced by regex-related instructions (`Match`, `NotMatch`, etc.) must be of type `Regex`.
4. **Regex recompilation**: Each `Regex` constant is recompiled during deserialization to verify that the regular expression is valid in the current runtime environment.

## Configuration Examples

### Using Source Code (Development)

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: auth-check
spec:
  requestPlugins:
    - enable: true
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
    - enable: true
      type: Dsl
      config:
        name: "auth-check"
        bytecode: "eyJ2ZXJzaW9uIjoxLCJjb2RlIjpbLi4uXSwi..."
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
  bytecode: "eyJ2ZXJzaW9uIjoxLCJjb2RlIjpbLi4uXSwi..."
```

## Related Documentation

- [DSL Overview](./README.md)
- [Sandbox Security Model](./sandbox-security.md)
- [Language Syntax Reference](./language-syntax.md)
