# D0008 — Dead PluginSession Log Hook

## Meta

- Status: resolved
- Severity: low
- Discovered in: B0043, `edgion-gateway/src/plugins/runtime/traits/session.rs`
- Category: plugin runtime API cleanup

## Evidence

Before the correction, `PluginSession::push_edgion_plugins_log` had zero call
sites. Its only source occurrences were the trait declaration, the production
`PingoraSessionAdapter` implementation, and a custom implementation in the
frozen Redis integration test. ExtensionRef logging exclusively used the
tokenized `start_edgion_plugins_log` and `push_to_edgion_plugins_log` API.

## Why deferred

Deleting the trait member and production implementation while preserving the
frozen test would have failed with Rust E0407 because the custom implementation
would have defined a method no longer present in the trait. The campaign could
not modify that existing integration-test file, so the otherwise mechanical
cleanup was deferred until the freeze was lifted.

## Resolution

- Resolved on 2026-09-21 after the integration-test freeze was lifted.
- Reusable contract: `skills/04-review/plugin-runtime/regression-guards/extension-ref-tokenized-plugin-log-contract.md`
