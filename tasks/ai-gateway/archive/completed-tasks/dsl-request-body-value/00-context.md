# Context

## Requirement summary

- Problem to solve:
  - `req.body` currently calls `read_request_body(None)` and converts the whole
    result to a DSL `Str`.
  - A disk-backed body is therefore copied into one contiguous allocation even
    when the script only needs a predicate such as `contains`.
  - Request-body inspection and mutation need one behavior across memory and
    file storage.
- Expected behavior:
  - `req.body` becomes an opaque request-scoped `Body` value.
  - Read-only operations use names and observable semantics aligned with the
    existing DSL string methods where possible: `len`, `is_empty`,
    `starts_with`, `ends_with`, `contains`, and regex `matches`.
  - Body-specific bounded reads, hashing, JSON selection, and mutations remain
    explicit.
  - JSON selection uses a bounded definite-path grammar and selective parsing
    rather than deserializing the complete body.
  - Storage errors and mutation failures fail closed.
- Explicitly out of scope:
  - A general filesystem, stream, cursor, iterator, or arbitrary I/O API in the
    DSL.
  - Response-body buffering or `resp.body`.
  - Full JSONPath filters, wildcards, unions, slices, or recursive descent in
    the first version.
  - Runtime-compiled regular expressions.
  - Unbounded captured matches, JSON results, replacement output, or segment
    counts.

## Current behavior

- `BuiltinId::ReqBody` is an async body builtin that returns `Value::Str`.
- `Value` lives in the leaf `edgion-dsl` crate, while the request body storage
  lives in `edgion-gateway`; the value therefore cannot own or reference a
  gateway type.
- `BodyStorageView` already owns the canonical memory/disk source, physical
  size cap, full materialization cache, streaming SHA-256 implementation, and
  pre-upstream mutation state.
- `BodyStorageView::replace_slice` currently calls `read_all`, so a range
  mutation materializes file-backed bodies.
- Existing string opcodes execute synchronously inside an async VM loop.
- Regex patterns for `matches`, `regex_find`, and `regex_replace` are required
  to be string literals and are compiled during configuration resolution.
- Existing `json_get` parses the complete input into `serde_json::Value` and
  uses simple dot notation.

## Initial impact assessment

- Possibly affected modules:
  - `edgion-dsl`: opaque value tag, bytecode, compiler lowering, validator, and
    bytecode-version change
  - `edgion-gateway/request_body`: immutable source-range segments and canonical
    chunk scanning
  - `edgion-gateway/plugins/runtime`: session operations and stock defaults
  - `edgion-gateway/plugins/http/dsl`: asynchronous text operation dispatch,
    error policy, JSON selection, mutation builtins, and tests
  - DSL user documentation and DslBody integration scenarios
- Key risks:
  - accidentally materializing disk-backed data in a convenience method
  - chunk-boundary bugs in literal, regex, UTF-8, and JSON processing
  - mutation publishing partial state after an error
  - segment explosion after repeated replacements
  - body handles escaping their request or being coerced into strings
  - body I/O errors passing through the configurable fail-open DSL policy
- Need a tasks document: yes

## Architectural constraints

- Only the proxied request body may use `BodyStorage`; there is no response-side
  equivalent.
- The Body value is readable in request-stage DSL execution and in
  upstream-response-filter DSL execution when an earlier request-stage
  `RequestBodyBuffer` populated the shared cache. A response-stage read never
  starts late capture. Body mutation is request-stage-only.
- `body-buffering` feature branching stays at the request-body transport
  boundary. The DSL schema and bytecode are flavor-agnostic.
- Request-body policy is resolved before the hot path.
- The DSL remains a bounded orchestration language; scanning and parsing are
  Rust capabilities, not a general language extension.
- Everything written to disk is English.

## Related Skills

- `skills/00-design-rules.md`
- `skills/01-architecture/02-gateway/dsl-engine.md`
- `skills/01-architecture/02-gateway/dsl-engine-security-model.md`
- `skills/03-coding/dsl/SKILL.md`
- `skills/03-coding/dsl/builtin-error-semantics.md`
- `skills/07-tasks/new_feature_work_flow.skill`
- `skills/07-tasks/new_feature_work_flow_subagents.md`
