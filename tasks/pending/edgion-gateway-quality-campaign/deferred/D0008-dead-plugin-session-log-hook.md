# D0008 — Dead PluginSession Log Hook

## Meta

- Status: ready-after-campaign-freeze
- Severity: low
- Discovered in: B0043, `edgion-gateway/src/plugins/runtime/traits/session.rs`
- Category: plugin runtime API cleanup

## Evidence

`PluginSession::push_edgion_plugins_log` has zero call sites. Its only repository
occurrences are the trait declaration, the production `PingoraSessionAdapter`
implementation, and a custom implementation in the frozen Redis integration
test. ExtensionRef logging exclusively uses the tokenized
`start_edgion_plugins_log` and `push_to_edgion_plugins_log` API.

## Why deferred

Deleting the trait member and production implementation while preserving the
frozen test would fail with Rust E0407 because the custom implementation would
define a method no longer present in the trait. The campaign cannot modify that
existing integration-test file. The cleanup is otherwise mechanical and should
run after the freeze is lifted.

## Canonical task

- `tasks/todo/remove-dead-plugin-session-log-hook.md`
