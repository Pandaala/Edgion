# DSL XML Primitives

## Meta

| Key | Value |
|-----|-------|
| Created | 2026-08-02 |
| Queue order | 2 |
| Status | in-progress |
| Type | feature |
| Priority | P2 |
| Issue | N/A |

## AI guide summary

Add safe, bounded XML parsing and construction primitives to EdgionDSL. The
API should expose enough XML structure for scripts to implement SOAP, legacy
integration, and application-specific mappings without imposing a universal
XML-to-JSON convention. Parsing and serialization must remain bounded,
transactional, and compatible with the existing request-body storage and
streaming-response boundaries.

## Requirements

- Define an XML value model that preserves namespace identity, attributes,
  child order, text, and repeated elements.
- Define DSL parse, traversal, construction, and serialization APIs.
- Charge parsing, traversal, and serialization against bounded VM work.
- Reuse the request-scoped `BodyStorage`; do not add another request cache.
- Enforce the resolved physical body cap before parsing.
- Enforce serialized-output bounds before publishing a mutation.
- Define explicit limits for depth, nodes, attributes, names, and text.
- Disable DTDs, external entities, external schemas, XInclude, network access,
  and unbounded entity expansion.
- Define deterministic namespace handling, escaping, and malformed-input
  behavior.
- Preserve transactional mutation: invalid or oversized output is never
  partially published.
- Preserve request replay, retry, mirroring, framing, and the canonical Edgion Pingora transport
  behavior.

## Scope

### In scope

- Bounded XML request-body parsing.
- Script-visible XML traversal and value extraction.
- Bounded XML element/attribute/text construction.
- Serialization into a request-body mutation with validation before publish.
- DSL compiler/bytecode validation, Gateway runtime support, tests, and EN/ZH
  user documentation.

### Out of scope

- A universal automatic XML-to-JSON mapping.
- XML Schema or WSDL validation and code generation.
- Network-backed entity or schema resolution.
- Transparent multipart, Protobuf, MessagePack, CSV, or YAML conversion.
- Full response-body buffering or a response-side XML API.

## Expected user scenarios

- Extract fields from SOAP requests for routing or authorization.
- Convert a bounded XML/SOAP request into application-specific JSON.
- Construct XML requests for legacy upstream services from JSON input.
- Normalize namespaces or envelope fields before proxying.

## Reviewed design

The immutable XML model, sixteen APIs, namespace/whitespace semantics, independent
hard bounds, nonrefundable work ledger and fixed body failure contract are frozen
in [xml/01-design.md](xml/01-design.md), approved by the independent plan reviewer.
The implementation uses the existing shared BodyStorage scanner lane and one
canonical overwrite after successful bounded serialization. No public resource
limit or response-body API was added.

## Affected modules

| Module | Impact |
|--------|--------|
| `edgion-dsl` | Parser-facing value model, compiler/bytecode, static validation, and builtin dispatch contract |
| `edgion-gateway` | Bounded request-body scanner, VM runtime, serializer, and transactional publication |
| `edgion-resources` | DSL configuration validation if the public API introduces resource-level limits |
| `edgion-tests/` | XML fixtures, body-cap, spill, replay, mutation, and security coverage |
| `docs/en/` and `docs/zh-CN/` | User-facing DSL API and limit documentation |

## Subtasks

| Subtask | Status | Output |
|------|------|------|
| Existing DSL and request-body capability inventory | completed | Current extension points and reuse boundaries |
| XML value model and API assessment | completed | Compared options with safety and compatibility conclusion |
| Limits, gas, and security design | completed | Hard bounds and disabled XML features |
| Compiler/bytecode and Gateway runtime design | completed | File-level change plan and data flow |
| Transactional serialization and publication | completed | Mutation and framing behavior |
| Unit, integration, security, and supported-build tests | completed | Malicious-input and regression matrix |
| Documentation and independent review | completed | EN/ZH contract and review closure |

## Acceptance status

Scoped implementation, EN/ZH documentation, reviewed code corrections and live
verification are complete. Fresh leaf/Gateway/runner checks pass 158/476/25;
canonical DSLBody passes 25/25, DslXmlDynamic passes all phases and cleanup, and
Transport passes 169/169 with required final gates. Full dependency audit/deny
still fail on pre-existing Wasmtime/Hickory advisories, reproduced against HEAD.
Overall acceptance remains pending that gate; see [xml/05-issues.md](xml/05-issues.md)
and [fresh evidence](xml/02-implementation.md). No advisory ignore, unrelated
upgrade, commit or push is included.

## Acceptance criteria

- The public DSL model and limits are documented before implementation.
- Entity expansion, DTDs, external resources, and network access are disabled
  and covered by safe-failure fixtures.
- Parsing, traversal, and serialization have bounded VM accounting.
- Invalid or oversized output cannot be published partially.
- Memory/spill behavior, body caps, replay, retry, mirroring, framing, and
  canonical Edgion Pingora behavior retain their existing invariants.
- Tests cover namespaces, attributes, child order, text/mixed content,
  repeated elements, malformed input, depth/node/text limits, and output caps.

## Related skills

- `skills/00-design-rules.md`
- `skills/01-architecture/02-gateway/dsl-engine.md`
- `skills/03-coding/dsl/SKILL.md`
- `skills/05-testing/01-integration-testing.md`
- `skills/11-doc/SKILL.md`

## Document index

| Document | Status | Notes |
|------|------|------|
| `dsl-xml-primitives.md` | in-progress | Requirements and acceptance status |
| `xml/00-context.md` | done | Existing mechanisms and original knowledge |
| `xml/01-design.md` | done | API, numeric bounds and budget, independent plan PASS |
| `xml/02-implementation.md` | in-progress | Implementation and fresh verification evidence |
| `xml/03-subtasks.md` | in-progress | XML-specific status; parent ExtProc evidence is preserved |
| `06-decisions.md` | — | Create when design decisions need a separate log |

## Decision log

| Date | Decision | Reason |
|------|----------|--------|
| 2026-08-02 | Expose XML structure rather than prescribe XML-to-JSON conversion | Namespace, ordering, mixed-content, and schema choices are application-specific |
| 2026-08-02 | Keep the feature request-side and bounded | The request-body cap and response-streaming invariants must remain authoritative |
