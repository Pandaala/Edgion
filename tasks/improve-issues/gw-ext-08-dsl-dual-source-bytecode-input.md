# gw-ext-08: DSL accepts operator-supplied bytecode that silently overrides `source`, a vestige that adds an audit hazard and attack surface

- **Category**: over-design
- **Severity**: P3 (downgraded from P2 in independent verification)
- **Scope**: local
- **Location**: `edgion-resources/src/resources/edgion_plugins/plugin_configs/dsl.rs:160-170` (`source`, `bytecode` fields), `dsl.rs:369-394` (`compile_and_validate`), `edgion-dsl/src/script.rs:39-64` (`resolve_script`), `edgion-dsl/src/validator.rs:537` (`compile_dsl_source` returns base64, which is then deserialized again), `edgion-gateway/src/plugins/http/dsl/plugin.rs:52-80` (`resolve_parts`)

## Summary
`DslConfig` accepts `source`, `bytecode` (base64 JSON `CompiledScript`, format version 5), or
both. When both are present, the bytecode is executed and the source is ignored, with no
check that the two agree. The original reason for a bytecode input, Controller-only
compilation with bytecode shipped to the Gateway, is obsolete according to the project's own
ADR. The Gateway now compiles and validates source itself. The remaining effects are
negative:
- Reviewers of an EdgionPlugins object may read and approve `source` while unrelated bytecode
  runs.
- The bytecode `Validator` becomes a security boundary against arbitrary hand-written opcode
  sequences, not just against compiler output.
- The project must maintain a versioned public bytecode format.

## Current behavior (evidence)
- `bytecode` is documented on the field as "populated by controller or via external tooling.
  Takes priority over `source` if both present" (`dsl.rs:167-170`). The Controller does
  *not* populate it. `dsl-design-decisions.md` states that "Controller validation does not
  replace source with bytecode", so the field comment is stale.
- `resolve_script` returns the deserialized `bytecode` as soon as it is present and never
  looks at `source` (`script.rs:39-55`). User docs confirm: "`bytecode` takes precedence,
  even if it is invalid (there is no fallback to source) ... the configured `source` remains
  serialized but is not executed" (`docs/en/.../dsl/bytecode.md:11, 130-133`).
- Because opcodes can be supplied directly, `Validator` (`edgion-dsl/src/validator.rs`, about
  560 lines) and helper heuristics such as `check_redis_link_names`/`has_jump_into_window`
  (`script.rs:14-37, 66-90`) must defend against adversarial control flow. Several regression
  guards exist largely because opcodes may be adversarial: `structured-bytecode-control-flow.md`,
  `builtin-argument-capacity-and-stack-validation.md` (source/bytecode parity), and
  `redis-link-check-positional-heuristic.md`.
- Even the source path round-trips through the bytecode wire format:
  `compile_dsl_source` serializes to base64 and `resolve_script` immediately deserializes it
  (`script.rs:57-61`, `validator.rs:537`).

## Industry comparison
- **Envoy Lua** (`inline_code` / `source_codes`), **Kong / APISIX Lua** plugins and
  serverless functions, and **Traefik Yaegi** plugins all accept *source only*. They compile
  inside the proxy, so the reviewed text is always the executed text.
- Proxy-Wasm is the industry's binary path. Binaries there are content-addressed and pinned
  by `sha256` (Edgion already requires this for URL sources), not placed beside a different,
  human-readable source.

## Impact
- Integrity and audit risk: in GitOps review, the visible `source` can differ completely from
  what runs. Nothing warns.
- Larger attack and maintenance surface: a stable public bytecode format, a version field,
  adversarial-input validation, and parity tests for two input paths, all without a
  functional benefit. Source compilation is bounded and happens once per configuration
  generation, never per request.

## Suggested direction
Pre-release, no compatibility burden:
1. Recommended: remove `DslConfig.bytecode` from the schema and CRD. Accept only `source`.
   Keep `CompiledScript` internal, and make `compile_dsl_source` return it directly instead
   of a base64 string. The validator then only needs to defend against compiler bugs, and
   can keep running as a debug and defense-in-depth assertion.
2. Minimum: reject configurations that set both fields, and fix the stale field comment.

## Notes
- `skills/04-review/dsl/intentional-design/dsl-design-decisions.md` is the locked engine
  decision. This finding does **not** reopen the choice of custom VM versus
  Rhai/mlua/rune/Boa. It targets only the input-representation surface. New evidence: the
  ADR's own "Current representation and transport" section states that the Controller-only
  compilation premise is obsolete, and that "A serializable bytecode format is supported;
  bytecode-only transport is not the current contract". Constraint (1), cross-process
  bytecode, therefore no longer has a consumer inside Edgion. Removing the bytecode input
  matches the ADR's re-evaluation clause ("Re-evaluate when field serialization,
  representation priority ... changes").
- The engine decision's remaining constraints (sandbox, no closures, no global state,
  async builtins) still favor the custom VM. See the final report's "considered but dropped"
  section.

## Verification (independent)

- **Verdict**: DOWNGRADED (P2 → P3)
- Claims hold: `edgion-resources/src/resources/edgion_plugins/plugin_configs/dsl.rs:167-170` documents bytecode precedence and a stale "populated by controller" comment; `edgion-dsl/src/script.rs:55-64` (`resolve_script`) prefers bytecode without comparing it to `source`.
- `skills/04-review/dsl/intentional-design/dsl-design-decisions.md:49-55,73-74` explicitly records source-or-bytecode with bytecode precedence as the *current contract* and says source removal "requires separate authorization". This is a documented design, so the finding is a product-direction proposal rather than a defect.
- Why lower: the EdgionPlugins author is already a trusted operator who can write arbitrary DSL source, so adversarial bytecode does not add a new privilege boundary; the Gateway still validates bytecode with `Validator` before use. The residual value is GitOps-review clarity plus maintenance surface. The "minimum" option (reject both fields set, fix the stale comment) is cheap and worth doing.

## Re-verification (feature-10-28)

- **Verdict**: STILL-VALID (severity P3 unchanged)
- `edgion-dsl` (all files), `dsl/plugin.rs` and `plugin_configs/dsl.rs` are unchanged since feature-08-30; dual `source`/`bytecode` inputs remain.
- `dsl-design-decisions.md` is unchanged.
