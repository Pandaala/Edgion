# Correct PluginRuntime Architecture Documentation

## Meta

| Key | Value |
|-----|-------|
| Created | 2026-09-07 |
| Status | pending |
| Type | documentation |
| Priority | P3 |
| Issue | N/A |
| Depends on | Current Gateway plugin-runtime ownership |

## AI guide summary

Align the plugin architecture and review knowledge set with the current runtime sharing, constructor, and clone contracts without changing runtime code.

## Requirements

- Verify every `PluginRuntime` construction and sharing call site before editing the architecture description.
- State that `PluginRuntime` is not `Clone` and is shared through `Arc<PluginRuntime>`; preserve the reason the former empty-runtime clone was removed.
- Remove the stale deprecated-forwarding-constructor claims from both `skills/01-architecture/02-gateway/05-plugin-system.md` and `skills/01-architecture/02-gateway/12-edgion-plugin-dev.md` after confirming current source exposes only scoped construction.
- Correct `skills/04-review/plugin-runtime/intentional-design/plugin-runtime-god-match.md` to describe the actual stream-runtime clone contract: clone-cheap `Arc`-backed chains rather than a no-clone rule.
- Keep Controller/Gateway compilation and immutable publication ownership accurate.
- Do not change runtime code or existing integration-test files as part of this documentation task.

## Exact documentation allow-list

- `skills/01-architecture/02-gateway/05-plugin-system.md`
- `skills/01-architecture/02-gateway/12-edgion-plugin-dev.md`
- `skills/04-review/plugin-runtime/intentional-design/plugin-runtime-god-match.md`

## Acceptance criteria

- The plugin-system page accurately states the no-`Clone` `PluginRuntime` and scoped constructor boundaries.
- The HTTP plugin-development page no longer claims removed forwarding constructors are present.
- The review precedent retains the empty-runtime-clone prohibition while accurately distinguishing clone-cheap stream runtimes.
- Relevant knowledge indexes remain consistent and `validate_agent_docs.py` passes.
- No integration-test issue is created because no runtime behavior changes.
