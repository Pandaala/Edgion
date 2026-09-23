# D0005 — Strict Gateway Process Config Unknown Fields

## Meta

- Status: resolved
- Severity: medium
- Discovered in: B0009, `edgion-gateway/src/cli/config.rs`
- Category: configuration correctness

## Evidence

The root Gateway process configuration and several nested process-config
objects accept unrelated unknown YAML keys. A known example is a misplaced
top-level `geoip` key, which can be silently ignored instead of rejecting the
operator's intended configuration.

## Why deferred

Uniform strictness affects a broader configuration compatibility and rollout
boundary than B0009's closed, one-field access-log option object. Applying
`deny_unknown_fields` selectively across unrelated structs would be incomplete
and could change startup acceptance without a coordinated design and inventory.

## Resolution

- `skills/04-review/deserialization/intentional-design/gateway-process-config-unknown-fields-advisory.md`
- The Gateway now reports ignored process-config fields with bounded startup WARN diagnostics. This record's evidence and deferral rationale describe the earlier B0009 state.
