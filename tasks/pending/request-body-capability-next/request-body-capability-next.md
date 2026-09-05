# Request Body Capability Next

## Meta

| Key | Value |
|-----|-------|
| Created | 2026-08-02 |
| Status | pending |
| Type | feature |
| Priority | P2 |
| Issue | N/A |

## AI guide summary

This queue contains the remaining request-body capabilities that require
future design and implementation: request-side ExtProc `STREAMED` processing
and bounded XML primitives for EdgionDSL. Both tasks must preserve the single
request-scoped body boundary, bounded resource use, transactional mutation, and
the invariant that proxied upstream responses remain streaming-only.

## Execution order

| Order | Task | Status | Purpose |
|------:|------|--------|---------|
| 1 | [ExtProc streamed request body](extproc-streamed-request-body.md) | blocked | Add bounded request-only `STREAMED` processing with strict per-chunk verdict gating |
| 2 | [DSL XML primitives](dsl-xml-primitives.md) | pending | Add bounded XML parsing and construction primitives for user-defined mappings |

## Shared constraints

- Use the canonical request-scoped `BodyStorage` only where complete-body
  access is required; do not add a second request-body cache.
- Keep the main proxied upstream response streaming-only.
- Enforce the resolved physical request-body cap and operation-specific bounds
  before publication.
- Preserve replay, retry, mirroring, generation consistency, and HTTP framing
  semantics.
- Fail closed for gateway invariant violations and keep client-facing failure
  bodies free of internal details.
- Keep `body-buffering` feature branches at the transport boundary.
- Compile reusable schemas, paths, limits, and other artifacts before the
  request hot path.

## Scope

### In scope

- Request-side ExtProc `STREAMED` processing.
- Bounded XML parsing, traversal, construction, and serialization for DSL.
- Unit, integration, build-flavor, and documentation work required by either
  feature.

### Out of scope

- ExtProc `BUFFERED_PARTIAL` or `FULL_DUPLEX_STREAMED`.
- ExtProc response-body or request-trailer processing.
- Full response-body buffering.
- Universal XML-to-JSON conversion rules.
- XML Schema/WSDL validation or network-backed entity/schema resolution.

## Document index

| Document | Status | Notes |
|------|------|------|
| `extproc-streamed-request-body.md` | blocked | Waiting for the canonical Pingora request-body transport interface |
| `dsl-xml-primitives.md` | pending | Requirements and acceptance criteria are recorded; design has not started |

## Decision log

| Date | Decision | Reason |
|------|----------|--------|
| 2026-08-02 | Keep the two capabilities as independent task records | Their transport and data-model risks are different and should be designed separately |
| 2026-08-02 | Keep response-body buffering outside this queue | The gateway response-streaming invariant remains unchanged |
