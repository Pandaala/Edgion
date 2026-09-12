# Preserve ExtProc Opaque Header Fidelity

## Meta

- Status: in-progress
- Priority: P2
- Category: protocol correctness

## Evidence

At `6bf48dcf`, `edgion-gateway/src/plugins/http/ext_proc/mutation.rs` and its canonical
session adapters have three related text/byte boundary problems:

- Request and response collectors use text accessors that discard non-text header values, although byte-preserving session accessors exist.
- `decoded_value` uses `from_utf8_lossy` for processor `raw_value`, changing opaque bytes.
- Conditional mutations infer header presence through text decoding, so an existing opaque value may be treated as absent.

These are distinct from ExtProc :path query projection and from framing/protected-header
policy. A local collector-only patch would not establish end-to-end fidelity.

## Acceptance

Read current rules and define the supported header byte contract across projection,
protobuf mutation, typed session setters/appenders and local-reply `PendingHeaderOp`.
Reuse canonical byte-preserving APIs and distinguish presence from text conversion.
Preserve header multiplicity, append conditions, removal order and protected/pseudo-header
restrictions. Do not broaden framing permissions or add lossy fallback behavior.

Add deterministic raw-header map and protobuf round-trip tests for request, response and
local-reply surfaces. Independently review the complete consumer chain and run scoped
tests/project guards. Do not modify existing integration tests; defer any needed new
end-to-end coverage as a separate task for later implementation.

## Approved implementation plan

The user authorized the shared session API and pending-header value changes.
Independent read-only plan review approved the following sequence:

1. Add canonical typed request append and response set/append APIs, direct live
   response-header presence, and typed `PendingHeaderOp` values. Preserve sensitive
   metadata, successful-write credential invalidation, and deferred invalid-string
   writes' existing successful no-op result.
2. Project ordinary and pseudo-header values through `raw_value`; borrow the
   request map and consume existing response byte snapshots. Validate returned
   bytes without lossy conversion. Preserve fallback, empty-value policy, all four
   actions, set-before-remove order, and protected headers.
3. Migrate affected queue construction and mock wiring sites. Cover raw maps,
   protobuf round trips, real in-memory adapters, sequential mutations, metadata,
   and fresh local replies with deterministic tests.
4. Update bilingual user documentation and the owning ExtProc regression guard.
   Track deployed wire coverage separately. Complete independent final review and
   repository checks before removing this active task. Do not commit or push.

Implementation source scope is Gateway `ctx.rs`, runtime session trait/adapter,
ExtProc mutation/plugin tests, and necessary queue test constructors in proxy
request-body/failure paths, error responses, and Wasm tests. No integration files
are changed. Knowledge context starts at `skills/SKILL.md`, design rules, review
routing, ExtProc knowledge, borrowed-header access, and sensitive-header ownership.
