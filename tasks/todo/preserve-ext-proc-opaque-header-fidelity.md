# Preserve ExtProc Opaque Header Fidelity

## Meta

- Status: pending
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
