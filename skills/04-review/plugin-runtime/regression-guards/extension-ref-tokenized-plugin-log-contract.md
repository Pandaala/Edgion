---
name: extension-ref-tokenized-plugin-log-contract
description: Use when reviewing PluginSession logging methods or ExtensionRef nested plugin-log ownership.
knowledge_type: regression-guards
---

# ExtensionRef tokenized plugin-log contract

## Conclusion and scope

`PluginSession` exposes one ExtensionRef nested-log write contract: start an
`EdgionPluginsLog` with `start_edgion_plugins_log`, then append each inner
`PluginLog` through `push_to_edgion_plugins_log` using the returned token. The
trait does not expose a second method that accepts an already assembled
`EdgionPluginsLog`.

The token stores the new entry's index in the request context's pending-log
vector, not a nesting depth. Callers use it before the outer plugin stage drains
that vector. The production adapter appends through `Vec::get_mut`; if the index
is absent, it ignores the log instead of panicking. This is a defensive fallback,
not a guarantee that a token remains valid across plugin stages.

This contract applies to request and response ExtensionRef dispatch. The
response-body path remains silent and uses `resolve_runtime_silent`, as required
by the plugin-system body-phase logging rules.

## Rationale and evidence

`ExtensionRefFilter::prepare_execution` starts the nested resource log before
dispatch. Its request and response stage loops append completed inner logs with
the same token, preserving the resource container even when stage execution is
nested. `PingoraSessionAdapter` maps those operations to
`EdgionHttpContext::start_pending_edgion_plugins_log` and
`pending_edgion_plugins_log_mut`.

The former `PluginSession::push_edgion_plugins_log` method had no caller. It
duplicated the active contract, forced every session implementation to provide
an unused method, and kept a matching one-shot context helper alive. D0008
removed the trait method, the production and test implementations, and the
unused context helper together.

The canonical phase ownership and body-path restrictions remain in
`skills/01-architecture/02-gateway/05-plugin-system.md`; this correction does
not introduce a new logging authority.

## Alternatives and limits

Keeping both whole-container and tokenized writes would leave two apparent
ownership models without a consumer for the whole-container form. Replacing the
tokenized API with whole-container assembly would require buffering inner logs
outside the request context and would change nested stage ownership; D0008 did
not make that broader design change.

The context still owns the `pending_edgion_plugins_logs` vector and terminal
stage draining. This rule does not change PluginLog rendering, access-log
serialization, ExtensionRef ordering, or failure policy.

## Verification and re-evaluation

Repository-wide source search after the correction found no
`push_edgion_plugins_log` or `push_pending_edgion_plugins_log` symbol. Rust
compile and lint gates verify that every `PluginSession` implementation follows
the remaining trait contract.

Re-evaluate if ExtensionRef nested logging stops being request-context-owned,
the token no longer identifies the pending resource log safely, or stage log
draining is redesigned. Do not reintroduce a whole-container trait method
without a concrete caller and a single documented ownership model.

## Resolution references

- D0008 / `remove-dead-plugin-session-log-hook`: dead plugin-session log hook
  removed on 2026-09-21 after the integration-test freeze was lifted.
- `correct-plugin-log-token-depth-docs`: removed stale depth and debug-panic
  claims from the `PluginSession` contract on 2026-09-21.
