# Bytecode Mode

## What Is Bytecode Mode

The DSL plugin supports two loading modes: `source` (source code) and `bytecode` (pre-compiled bytecode).

**Source mode**: The Controller compiles and statically validates source when checking the configuration, without replacing the serialized `source` field. The Gateway receives that configuration and independently compiles and validates the source when constructing the plugin.

**Bytecode mode**: Supply pre-compiled bytecode in the configuration. Both Controller validation and Gateway construction decode it, check its version, and statically validate it. Bytecode skips source parsing and compilation; it does not skip validation or Gateway VM initialization.

At least one of `source` and `bytecode` is required. Both may be present; `bytecode` takes precedence, even if it is invalid (there is no fallback to source). Both configured fields remain serialized. Compilation and VM construction happen during configuration processing, not per request.

## Bytecode Format

Bytecode is transmitted as a Base64-encoded JSON string.

### JSON Structure

```json
{
  "version": 5,
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
| `version` | integer | Bytecode format version. Current version is `5`. |
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
- `JsonSelect`: a static named multi-path plan used by `.json_select()`

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
- If the limit is exceeded, Controller validation reports an error, and independent Gateway validation also rejects the invalid artifact.

## Version Compatibility

Bytecode carries a version number in the `version` field, which is strictly validated during deserialization.

- If the version does not match, Controller validation reports a version error; Gateway validation independently rejects it before VM initialization.
- If the bytecode format version changes after an Edgion upgrade, existing bytecode must be recompiled.
- It is recommended to bind bytecode to a specific Edgion version in your CI/CD pipeline to avoid version drift.

Current version: `BYTECODE_VERSION = 5`

Version 5 removes the whole-body text-read opcode and requires explicit
`read(offset, max_bytes)` calls with `max_bytes <= maxStringLen`. Version 4
and all older or unversioned payloads are rejected. Replace whole-body reads
with explicit ranges and recompile with the target Edgion version.

## Security Validation

After bounded decoding and version checks, the shared validator performs the following static checks in both binaries. If any check fails, the bytecode is rejected:

1. **Version match**: The `version` field matches the version expected by the runtime.
2. **Constant index bounds**: All constant indices referenced by `LoadConst` instructions fall within the bounds of the constant pool.
3. **Instruction/constant type match**: Value loads accept only `Str`/`Int`; `.matches()`, string regex operations, `.json_get()`, and `.json_select()` require `StreamingRegex`, `Regex`, `JsonPath`, and `JsonSelect` constants respectively.
4. **Regex reconstruction**: Regex constants are recompiled with bounded automaton size to verify syntax and resource limits in the current runtime.
5. **JSONPath validation**: Every `JsonPath` constant is parsed again and must remain a definite path.
6. **JSON multi-select validation**: Every selector is non-empty and bounded,
   has unique output names, and contains only definite paths.

## Configuration Examples

### Using Source Code (Development)

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: auth-check
spec:
  requestPlugins:
    - type: Dsl
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
    - type: Dsl
      config:
        name: "auth-check"
        bytecode: "BASE64_ENCODED_BYTECODE_V4"
```

### Providing Both Source and Bytecode

When both fields are configured, `bytecode` takes effect and the configured `source` remains serialized but is not executed. The bytecode may separately contain source metadata embedded when it was compiled; loading does not copy the configuration's `source` into that metadata.

```yaml
config:
  name: "auth-check"
  source: |
    let token = req.header("Authorization")
    if token == nil {
      return deny(401, "Unauthorized")
    }
  bytecode: "BASE64_ENCODED_BYTECODE_V4"
```

## Related Documentation

- [DSL Overview](./README.md)
- [Sandbox Security Model](./sandbox-security.md)
- [Language Syntax Reference](./language-syntax.md)
