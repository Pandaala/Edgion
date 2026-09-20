# Remove Dead PluginSession Log Hook

## Status

Ready after the gateway quality campaign's integration-test freeze is lifted.

## Problem

`PluginSession::push_edgion_plugins_log` is a required trait method with no call
sites. It duplicates the active tokenized ExtensionRef logging contract and
forces every `PluginSession` implementation to carry dead API surface.

## Exact scope

- `edgion-gateway/src/plugins/runtime/traits/session.rs`
- `edgion-gateway/src/plugins/runtime/session_adapter.rs`
- `edgion-tests/tests/dsl_redis_rate_limit.rs`

## Constraints

- Remove the trait declaration and both implementations together so the
  compiler-enforced implementation set remains valid.
- Remove imports made unused by the deletion.
- Preserve the active `start_edgion_plugins_log` and
  `push_to_edgion_plugins_log` tokenized contract and all PluginLog rendering.
- Do not change ExtensionRef stage dispatch, access-log ownership, failure
  policy, body handling, schema, or transport behavior.

## Acceptance direction

Repository-wide symbol search finds no `push_edgion_plugins_log`; Gateway trait,
ExtensionRef, and Redis DSL integration tests compile and pass; full repository
checks remain clean apart from accepted pre-existing warnings.
