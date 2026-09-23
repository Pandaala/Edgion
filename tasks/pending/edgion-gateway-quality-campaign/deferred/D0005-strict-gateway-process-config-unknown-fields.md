# D0005 — Strict Gateway Process Config Unknown Fields

## Meta

- Status: resolved
- Severity: medium
- Discovered in: B0009, `edgion-gateway/src/cli/config.rs`
- Category: configuration correctness

## Evidence

At discovery, the root Gateway process configuration and several nested
process-config objects accepted unrelated unknown YAML keys without a
diagnostic. A misplaced top-level `geoip` key was one example: it left the
operator's intended configuration unapplied.

## Why deferred

Uniform strictness affects a broader configuration compatibility and rollout
boundary than B0009's closed, one-field access-log option object. Applying
`deny_unknown_fields` selectively across unrelated structs would be incomplete
and could change startup acceptance without a coordinated design and inventory.

## Resolution

- Ordinary unknown Gateway process-config fields emit an ERROR-level startup
  diagnostic and do not block startup. Existing strict subtrees remain strict.
- The reusable decision is in
  `skills/04-review/deserialization/intentional-design/gateway-process-config-unknown-fields-advisory.md`.
- This record's evidence and deferral rationale describe the earlier B0009 state.
