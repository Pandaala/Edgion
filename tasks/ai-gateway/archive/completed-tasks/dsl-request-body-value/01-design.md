# Design

## Solution assessment

| Dimension | Option A: first-class Body value | Option B: `req.body_*` builtins | Option C: general BodyReader |
|------|--------|--------|--------|
| Change difficulty | High | Medium | Very high |
| Impact scope | DSL core, VM, session, storage | VM, session, storage | DSL language, VM, session, storage |
| Safety | Strong with opaque token and bounded operations | Strong but API grows inconsistently | Weakest; exposes stateful I/O semantics |
| Implementation value | Highest; familiar and extensible | Good but duplicates String vocabulary | Low for gateway use cases |
| Test difficulty | High | Medium | Very high |
| Compatibility | Intentional pre-release breaking change | Can preserve old `req.body` | Breaking and conceptually larger |
| Rollback ability | Bytecode version and isolated value tag make rollback clear | Easiest | Hardest |

### Option A (recommended)

- Core idea:
  - `req.body` evaluates to `Value::Body(RequestBody)`, an opaque zero-data tag.
  - The async VM dispatches shared read-only text opcodes over either `Str` or
    `Body`.
  - Gateway-internal scanners consume chunks from a string, memory source, disk
    source, or immutable mutation segments through one canonical interface.
- Strengths:
  - Script authors use one vocabulary for common text predicates.
  - The DSL value never owns a file, cursor, or body allocation.
  - Storage choice remains invisible and can change after the memory threshold
    without changing DSL behavior.
  - New bounded body capabilities can be added without growing a general I/O
    language.
- Risks:
  - Existing scripts that treat `req.body` as a string must be updated.
  - Existing synchronous opcodes become conditionally asynchronous.
  - Cross-type equality needs an explicit policy.
- Test requirements:
  - every operation against string, memory body, disk body, and chunk-boundary
    cases
  - bytecode revalidation and version rejection
  - fail-close I/O paths
  - mutation replay across upstream retries

### Option B (alternative)

- Core idea: keep `req.body` as a full string and add functions such as
  `req.body_contains`.
- Reason for rejection:
  - It preserves the allocation trap as the shortest API.
  - It creates a second naming scheme for operations already understood as
    string methods.
  - It makes composition and future selective JSON access less coherent.

### Option C (alternative)

- Core idea: expose a cursor-like reader with `read`, `seek`, and iteration.
- Reason for rejection:
  - Shared cursor state is difficult to make deterministic across multiple
    plugins and retries.
  - It expands the DSL into general I/O and makes work budgets script-dependent.
  - It exposes storage mechanics rather than gateway body operations.

## Assessment conclusion

- Recommended option: Option A.
- Reason: it gives the smallest coherent user model while retaining strict
  resource and storage boundaries.
- Open assumptions:
  - A direct `regex-automata` dependency can reproduce the accepted syntax and
    size ceiling of the existing compiled `regex` patterns for boolean
    streaming matches.
  - Pingora replay can be extended from `Bytes` mutation chunks to immutable
    source-range segments without changing retry semantics.
  - A selective JSON visitor can preserve the existing scalar conversions and
    last-duplicate-key behavior while retaining only the selected bounded
    subtree.
- Main reasons rejected options were dropped:
  - Builtin-only APIs preserve the wrong default.
  - Reader APIs expose state and unbounded composition that the DSL should not
    own.

## Public DSL contract

### Value and coercion

- `req.body` returns type `Body`.
- A `Body` is truthy, request-scoped, cloneable as a small token, and contains
  no bytes or host reference.
- Read-only Body operations are valid in request plugins and in upstream
  response-filter plugins when request-stage capture already populated the
  shared cache. Response-stage evaluation never starts capture. Mutations are
  request-stage-only and are rejected during stage-aware configuration
  validation.
- `Body` is not accepted by implicit string coercion, concatenation,
  interpolation, headers, logging, map keys, or `to_string`.
- Full conversion is explicit: `req.body.read_all()`. It returns `Str` and
  enforces `VmLimits::max_string_len`; it is intended only for small bodies.
- `req.body == "literal"` and `"literal" == req.body` perform streaming text
  equality. Other cross-type comparisons remain false or retain their current
  type error.
- `Body == Body` is true because the only Body tag identifies the same current
  request body. `Body != Body` is therefore false. Ordering a Body is a type
  error.
- Shared text methods preserve the current `req.body` string semantics without
  allocating that string. A streaming lossy UTF-8 decoder carries incomplete
  code points across storage chunks and emits replacement characters with the
  same rules as `String::from_utf8_lossy`. Equality and text predicates consume
  that logical text stream. Raw hashing, byte-range reads, and mutations still
  address the original request bytes.

### Shared read-only text methods

The following method names have the same logical text result for `Str` and
`Body`:

| Method | Result | Notes |
|------|------|------|
| `len()` | `Int` | Unicode scalar count, matching the existing DSL `len(Str)` |
| `is_empty()` | `Bool` | equivalent to `len() == 0` |
| `starts_with(s)` | `Bool` | string argument; empty pattern is true |
| `ends_with(s)` | `Bool` | string argument; empty pattern is true |
| `contains(s)` | `Bool` | string argument; empty pattern is true |
| `matches("regex")` | `Bool` | compile-time literal pattern |

`contains`, prefixes, suffixes, equality, and regex matching must work when a
match crosses any storage chunk boundary. Invalid receiver or argument types
retain the existing nil-friendly `false` behavior.

Body regex matching consumes the same streaming lossy UTF-8 view, so its result
matches the string produced by the old materializing `req.body` behavior even
when the original body contains invalid UTF-8.

The shared streaming regex dialect excludes constructs that the incremental
DFA cannot execute with identical semantics. In particular, Unicode word
boundaries are rejected at compile time; scripts can use an explicit ASCII
boundary such as `(?-u:\b)`. The compiler applies this validation to every
`.matches()` pattern because the dynamically typed receiver may be either
String or Body. There is no runtime fallback to `read_all`.

`regex_find` is not generalized to Body in the first version because returning
the matched text adds capture-size and offset semantics. `regex_replace`
remains a string function; Body literal mutation uses the explicit method
below.

### Body-only read methods

| Method | Result | Bound |
|------|------|------|
| `byte_len()` | `Int` | raw captured byte length; no scan |
| `read(offset, max_bytes)` | `Str` | selects the raw byte range first, then lossily decodes that isolated slice; the result must fit `max_string_len` bytes |
| `read_all()` | `Str` | body must fit `max_string_len` |
| `sha256()` | `Str` | fixed 64-byte lowercase hex |
| `json_get("$.a[0].b")` | scalar, bounded serialized subtree, or `nil` | path is a compile-time literal; retained result fits `max_string_len` |

`read` uses an independent positional range and never changes replay or another
operation's position.

### Body-only mutation methods

These methods mutate the request body transactionally and return `nil` on
success:

| Method | Effect |
|------|------|
| `clear()` | replace the body with empty content |
| `append(s)` | add bytes after the current body |
| `prepend(s)` | add bytes before the current body |
| `remove(start, end)` | remove the half-open byte range |
| `replace(from, to)` | replace all literal byte occurrences |

Mutation is request-stage only. Invalid ranges or a result above the physical
body cap fail closed. `replace("", value)` is rejected at compile/runtime
rather than creating insertion-at-every-boundary semantics. Literal replacement
is non-overlapping and leftmost-first. Body mutation searches the original
UTF-8 pattern bytes in the raw body so untouched invalid bytes are preserved;
String replacement continues to operate on valid DSL strings.

Repeated mutations operate on the latest immutable snapshot. A mutation is
published only after all input has been scanned and output length and segment
count have passed their limits.

## Text operation architecture

### Semantic unification

String-like behavior is unified at the operation layer, not by pretending a
file is a Rust `str`.

A gateway-internal `TextChunkSource` abstraction feeds immutable byte chunks to
stateful scanners. Implementations are:

- `StrSource`, which feeds one borrowed chunk;
- `BodySnapshot`, which feeds memory bytes, disk ranges, and inline mutation
  segments in logical order.

The same scanner implementation is used for both sources:

- literal prefix/suffix/equality scanner
- streaming lossy UTF-8 adapter and character counter
- streaming substring scanner
- streaming regex DFA
- SHA-256 scanner
- selective JSON scanner

The abstraction has no public cursor. Each operation creates its own scan
state, so concurrent reads and Pingora replay are independent. The
flavor-agnostic `RequestBodyState` facade owns the operation API; only its
backend branches between fork and stock implementations.

### Disk execution

- Disk ranges use `FileExt::read_at`; no shared file offset is mutated.
- One logical disk scan moves an owned immutable snapshot and owned scanner into
  one `spawn_blocking` job instead of spawning once per chunk. No borrowed
  visitor crosses the `'static` task boundary.
- The chunk size is an internal constant and does not alter semantics.
- Memory and inline segments are consumed without copying.
- A process-wide 32-permit disk-scan semaphore caps concurrent blocking jobs. Dropping
  the async waiter sets a cancellation flag; scanners check it between chunks
  and perform no publication after cancellation.

### VM representation

- Add `Value::Body(BodyValue::Request)` in `edgion-dsl`. `BodyValue` is a small
  serializable-free runtime tag; it does not reference gateway state.
- Preserve the 24-byte `Value` layout invariant.
- `BuiltinId::ReqBody` performs a metadata-only availability check and then
  pushes the tag; it neither starts capture nor calls `read_all`. For request
  plugins, static body-use detection makes the existing request body gate
  complete capture before VM execution. An upstream response filter only reads
  an already-cached request body. A known bodyless request validates as length
  zero; a declared or unknown body with no cache is a fixed fail-close error.
- Shared text opcodes dispatch asynchronously in `Vm::execute`. String
  receivers use `StrSource`; Body receivers delegate to `PluginSession`.
- Replace or wrap infallible string coercion at every host boundary so a Body in
  a header, log, interpolation, outbound call, map key, or unrelated builtin
  raises a type error. `Display` is diagnostic-only and is never a coercion
  path.
- Add explicit opcodes/builtin IDs for length, emptiness, reads, hashing, JSON,
  and mutations. Increment `BYTECODE_VERSION`; old serialized bytecode is
  rejected because `req.body` changed type.
- The controller-side compiler validates literal regex and JSON paths without
  depending on gateway storage.
- `CompiledScript` exposes body-read and body-mutation introspection. The
  resource's stage-aware validator and the gateway response-plugin factory both
  reject mutation opcodes outside the request stage.
- `DslPlugin::requires_materialized_request_body()` returns `false`. The
  existing body gate still performs fail-close complete capture and business
  limit enforcement, but it no longer defeats the Body value by calling
  `read_all` before VM execution.

## Storage and mutation model

Replace `MutationBody { chunks: Vec<Bytes> }` with an immutable logical body:

```text
BodySnapshot
  Segment::SourceRange { source, offset, len }
  Segment::Inline(Bytes)
```

- A source is immutable memory bytes or an `Arc<File>` with a fixed length.
- Initial memory and disk bodies are represented as one source range.
- Append and prepend add one inline segment.
- Remove slices or drops source ranges without reading their contents.
- Literal replace scans source chunks and emits unmatched source ranges plus
  inline replacements; unchanged file ranges remain file-backed.
- Adjacent compatible ranges are normalized after each mutation.
- Empty segments are removed.
- A hard limit of 4,096 logical segments prevents adversarial replacement or
  repeated mutation from creating unbounded metadata.
- Length arithmetic uses checked `u64`; DSL offsets are non-negative `i64` and
  are checked before conversion.
- Replay reads the immutable segment snapshot and keeps its own logical cursor.
  A new mutation invalidates the adapter's cached replay snapshot exactly as
  the current mutation generation does.

The existing `materialized` cache applies only to the original unmodified
source. A mutated body is never inserted into that cache. `read_all` may
materialize the latest snapshot only for an explicit full-read consumer.

## Selective JSON design

### Accepted path grammar

The first version accepts only definite paths:

- root: `$`
- object member: `.name`
- quoted object member: `["name.with.dots"]`
- non-negative array index: `[0]`

The path must be a compile-time string literal. Maximum encoded path length and
segment depth reuse the existing regex/string and depth ceilings where
appropriate.

Wildcards, recursive descent, filters, unions, slices, negative indexes, and
script-supplied dynamic paths are rejected during compilation.

### Parsing behavior

- A streaming JSON parser consumes the entire document so malformed trailing
  input cannot be accepted after an early match.
- Unselected values are validated and skipped without allocating a
  `serde_json::Value`.
- Only the latest value at the definite path is retained, preserving the
  existing last-duplicate-key behavior. An oversized earlier candidate is
  represented as an `Oversized` state without retaining its bytes; a later
  candidate replaces that state. The size error is emitted only if the final
  candidate is oversized. This rule applies to duplicate ancestors as well as
  the final key.
- Scalars map to existing DSL values.
- A selected array or object is serialized into a bounded buffer; exceeding
  `max_string_len` raises the existing string-limit error.
- A missing path, type mismatch during traversal, or invalid request JSON
  returns `nil`, matching the current data-error contract.
- Storage I/O failure is not malformed client data and follows the fixed
  fail-close policy.

The existing `json_get(string, path)` remains available and is refactored onto
the same selector. Its path argument becomes a compile-time definite JSONPath
literal, so existing dot-only calls such as `"user.name"` must be changed to
`"$.user.name"`. Edgion is pre-release, so no second legacy dot-path parser or
alias is retained. A `Str` method form may be added only if it does not create
another parser.

## Error and failure policy

- Invalid body content that a script can reasonably branch on:
  - no literal/regex match → `false`
  - missing/invalid JSON → `nil`
- DSL type misuse follows the current nil-friendly method behavior where
  already established. Mutation argument type errors remain typed script
  errors; invalid byte ranges are hard Body-access failures because applying
  the requested body transformation is unsafe.
- Body capture, disk I/O, incomplete storage, physical-cap overflow, segment
  overflow, and mutation publication errors are hard fail-close failures.
- Pre-VM body-gate failures retain the canonical body-gate mapping: a body
  limit returns 413, timeout returns 408, and refusal or other capture failure
  returns 500, all with an empty response body.
- VM Body-access failures bypass `DslErrorPolicy::Ignore` and return a generic
  fail-close 500 response through `PluginRunningResult::ErrResponse`.
- Internal details are recorded only through the bounded, redacted plugin error
  path; the client body contains no filesystem path or dependency detail.

## Resource limits

- The gateway physical body cap remains the maximum bytes scanned by one body
  operation.
- VM call and step limits bound how many operations a script can invoke.
- `VmState` also owns a cumulative body-scan byte budget. A source operation
  receives the remaining budget, stops before exceeding it, and returns the
  source bytes fetched in chunks of at most 64 KiB for accounting. An
  early-exit scanner charges its complete final fetched chunk. The initial
  budget is
  `resolved_physical_cap * maxBodyScanPasses`, using checked arithmetic, so a
  body expanded after capture still has the promised scan allowance.
  `maxBodyScanPasses` defaults to 4 and is validated in `1..=16`; it is resolved
  with the DSL configuration rather than read from global state on the hot
  path.
- Before a potentially blocking scan starts, the VM reserves its remaining
  scan allowance. Normal completion refunds the unconsumed portion reported by
  the scanner. Cancellation receives no refund because the VM execution no
  longer exists. Metadata-only `byte_len` and empty checks consume zero bytes;
  early matches charge complete source chunks fetched, including the final
  chunk in which they stop; I/O failures and rolled-back scanning mutations
  retain the charge for chunks fetched before failure.
- Literal patterns are bounded by `max_string_len`.
- Regex patterns remain compile-time literals with the existing pattern and DFA
  memory limits. A direct `regex-automata` dependency is used in both the leaf
  compiler crate and gateway runtime so stream compatibility is rejected before
  bytecode reaches the hot path.
- JSON path depth is at most 32 and selected output is at most
  `max_string_len`. Encoded JSON paths are at most 1,024 UTF-8 bytes.
- Mutation output is at most the physical body cap.
- Mutation segment count has a 4,096-segment hard cap. Exceeding it fails
  closed rather than materializing as a fallback.
- No operation silently truncates. `read` returns no more source bytes than its
  explicit `max_bytes`; `read_all`, `read`, and JSON selected output report
  string-size errors when their resulting DSL string is too large.

## Delivery sequence

The feature is implemented in dependency order, with each milestone compiling
and carrying its own tests before the next begins:

1. **Body token and read-only text source**: bytecode version, capture-only DSL
   gate, stock-safe session facade, scan budget, lossy UTF-8 adapter, shared
   equality/prefix/suffix/contains/regex, bounded reads, and SHA-256.
2. **Immutable mutation snapshots**: source-range segments, replay support,
   append/prepend/remove/literal replace, normalization, and atomic limits.
3. **Selective JSON**: definite-path compiler, streaming selector, existing
   `json_get` migration, and bounded selected-value conversion.
4. **External contract**: documentation, DslBody integration coverage, stock
   build, and repository checks.

No milestone may temporarily restore implicit full-body conversion. If the
streaming regex or selective JSON assumptions fail, implementation returns to
this design phase instead of adding a materializing fallback.
The milestones are internal checkpoints only: the breaking public Body
contract is delivered atomically after all four are complete.

## Code change plan

### DSL core

- Paths:
  - workspace and crate `Cargo.toml` files for the direct `regex-automata`
    dependency
  - `edgion-dsl/src/value.rs`
  - `edgion-dsl/src/bytecode.rs`
  - `edgion-dsl/src/compiler/expr.rs`
  - `edgion-dsl/src/compiler/builtins.rs`
  - `edgion-dsl/src/validator.rs`
- Change:
  - add the opaque Body tag, bytecode operations, method lowering, validation,
    body-use introspection, and bytecode version bump
  - add a pure definite-JSON-path parser shared by controller and gateway

### Request-body storage

- Paths:
  - `edgion-gateway/src/request_body/storage.rs`
  - `edgion-gateway/src/request_body/state.rs`
  - `edgion-gateway/src/request_body/state/enabled.rs`
  - `edgion-gateway/src/request_body/state/stock.rs`
- Change:
  - introduce immutable source-range snapshots
  - add canonical positional scanning and transactional mutations
  - update replay to consume logical segments without full materialization

### Plugin session boundary

- Paths:
  - `edgion-gateway/src/plugins/runtime/traits/session.rs`
  - `edgion-gateway/src/plugins/runtime/session_adapter.rs`
- Change:
  - expose bounded request-body operations required by the VM
  - keep stock defaults flavor-agnostic and fail closed as unimplemented

### DSL runtime

- Paths:
  - `edgion-gateway/src/plugins/http/dsl/lang/vm.rs`
  - `edgion-gateway/src/plugins/http/dsl/lang/builtins.rs`
  - a focused text/body operations module under the same directory
  - `edgion-gateway/src/plugins/http/dsl/plugin.rs`
  - `edgion-resources/src/resources/edgion_plugins/plugin_configs/dsl.rs`
- Change:
  - dispatch shared text semantics
  - implement fixed fail-close body errors
  - implement selective JSON result conversion
  - resolve and validate the per-execution body-scan pass budget

### Documentation and tests

- Paths:
  - `docs/en/user-guide/http-route/filters/edgion-plugins/dsl/`
  - `skills/01-architecture/02-gateway/dsl-engine.md`
  - `skills/01-architecture/02-gateway/dsl-engine-security-model.md`
  - `skills/03-coding/dsl/builtin-error-semantics.md`
  - DSL compiler/validator/unit tests
  - request-body storage and adapter tests
  - `edgion-gateway/src/plugins/http/dsl/tests/`
  - `edgion-tests/integration/resources/EdgionPlugins/DslBody/`
- Change:
  - document the breaking contract and locked failure semantics
  - cover external behavior through the existing DslBody integration resource

## Data flow

1. Controller compilation parses method syntax and precompiles regex/JSON path
   constants.
2. Gateway bytecode validation rechecks every constant reference and operation.
3. The request body gate captures once into the request-scoped `BodyStorage`.
4. Evaluating `req.body` pushes only an opaque request-body token.
5. A text/body opcode asks the session adapter for a fresh snapshot operation.
6. The snapshot scanner consumes logical segments from memory and/or disk.
7. Read operations return bounded DSL values; mutations atomically publish a
   new snapshot.
8. Pingora replay reads the latest immutable snapshot with its own cursor.

## Test design

### Unit tests

- DSL core:
  - Body value layout, truthiness, type name, and forbidden coercion
  - method arity and receiver-independent lowering
  - literal-only regex and JSON path validation
  - stale bytecode version and malformed new opcodes
- Scanner:
  - empty and one-byte patterns
  - match at every chunk boundary
  - prefix/suffix/equality length mismatch
  - regex anchors, Unicode, invalid UTF-8, and end-of-input transition
- Storage:
  - identical results for memory and forced spill
  - scans do not populate the `read_all` cache
  - independent concurrent positional scans
  - append/prepend/remove/replace preserve disk-backed untouched ranges
  - output cap, segment cap, overflow, invalid range, and atomic rollback
  - replay and retry after every mutation
- JSON:
  - each accepted path form
  - chunk boundaries inside strings, escapes, numbers, and structural tokens
  - missing/type mismatch/invalid trailing JSON
  - duplicate keys select the last value
  - unselected huge subtree does not become a DOM
  - selected output limit
- VM:
  - shared String/Body method parity
  - equality symmetry
  - no implicit Body conversion
  - fixed fail-close body I/O despite `errorPolicy: Ignore`
  - every implicit string-coercion boundary rejects Body
- Stock flavor:
  - a known bodyless request yields an empty Body
  - a script statically referencing `req.body` fails at the body gate when
    capture is unsupported, even if a runtime branch would not evaluate it
  - response-stage read-only access uses an earlier request cache, never starts
    capture, and missing declared-body storage fails closed

### Integration tests

- Extend DslBody with:
  - small memory-backed predicates and JSON selection
  - forced-spill contains, cross-chunk regex, and JSON selection
  - append/prepend/remove/replace observed by the upstream
  - repeated upstream replay or retry where supported by the existing harness
  - malformed JSON fallback
  - physical-cap and unsupported stock behavior

## Risks and compatibility

- Compatibility handling:
  - Edgion is pre-release; no alias or legacy `req.body` string mode is kept.
  - Source scripts must use `read_all()` only when they intentionally need a
    bounded string.
  - Serialized bytecode version is incremented so old semantics cannot be
    silently executed as new semantics.
- Failure rollback points:
  - The feature is divided into DSL token, read-only scanners, JSON selection,
    and mutation snapshots. Each layer is independently testable.
  - No partial mutation is published.
  - Segment-limit overflow never falls back to materialization.
