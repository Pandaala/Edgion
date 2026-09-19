# Enforce Gateway Process Config Unknown-Field Policy

## Meta

- Status: pending
- Type: correctness
- Priority: P2
- Discovered in: Gateway quality campaign B0009

## Evidence

The root Gateway process configuration and several nested process-config
objects currently accept unknown YAML keys. This can turn misplaced or
misspelled settings into silent no-ops; a documented example is a top-level
`geoip` key placed outside its owning object.

## Why deferred

Strictness is a process-config-wide compatibility and rollout decision. B0009
only closes the proven `access_log_options.includeRawUri` typo trap on a one-field
object; opportunistically tightening unrelated structs would be partial and
outside that batch's anchor scope.

## Candidate next step

Inventory every Gateway process-config struct and documented key, define a
uniform unknown-field policy, add clean-and-typo tests at each owning boundary,
then update examples and startup diagnostics in one coordinated task.
