# D0096 — Align Upstream Response Stage Documentation

## Meta

- Status: pending
- Severity: medium (architecture guidance drift)
- Discovered in: B0143 upstream-response trait analysis
- Category: documentation
- Canonical task: [Align upstream response stage documentation](../../../todo/align-upstream-response-stage-documentation.md)

## Evidence

The plugin-system guide says the hook runs after the upstream response completes, and the plugin-development guide calls it a response-end phase after the full response ends. Runtime dispatch instead invokes it for the final response headers before downstream commit and body streaming.

## Why deferred

B0143 owns one trait file. The two shared guides require their own cross-document accuracy review so examples and phase ordering remain consistent.

## Next step

Follow the canonical task; no integration-test change is required.
