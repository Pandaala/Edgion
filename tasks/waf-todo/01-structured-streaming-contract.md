# Freeze the Structured Streaming Contract

## Meta

| Key | Value |
|-----|-------|
| Status | pending |
| Priority | P1 |
| Depends on | Existing Phase 2 execution classifier |

## Goal

Define the exact bounded semantics that allow a Phase 2 rule unit to use
`StreamingStructured`. The compiler must never classify a rule as streaming
when incremental execution can change its SecLang result.

## Work

- Inventory the Phase 2 JSON and XML targets used by the admitted CRS profile.
- Define which operators and transformations can consume incrementally
  published scalar values.
- Define duplicate-key, array-index, path, escaping, malformed-input, and EOS
  behavior.
- Separate monotone early-decision rules from rules that must finalize at EOS.
- Define the independent limits for depth, token bytes, decoded bytes, entries,
  path bytes, retained bytes, and parser work.
- Specify downgrade rules from `StreamingStructured` to `BoundedSnapshot`.
- Require `Unsupported` when no bounded exact interpretation exists.

## Tests

- Table-driven classifier tests for every accepted and rejected target,
  operator, transformation, chain, negation, and stateful-action combination.
- Tests proving that an unsupported structured combination cannot silently
  enter either the raw or structured streaming lane.

## Exit gate

The compiler capability matrix is documented and executable tests prove every
Phase 2 rule unit receives one conservative execution class.
