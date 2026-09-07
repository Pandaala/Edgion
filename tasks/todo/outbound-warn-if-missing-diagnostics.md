# Emit Bounded Diagnostics for Outbound Warn-if-missing Fields

## Meta

| Key | Value |
|-----|-------|
| Created | 2026-09-07 |
| Status | pending |
| Type | correctness |
| Priority | P3 |
| Issue | N/A |
| Depends on | Shared outbound request diagnostic ownership |

## AI guide summary

Make `PresencePolicy::WarnIfMissing` continue request construction while returning one bounded, value-free diagnostic through the canonical PluginLog/access-log path.

## Requirements

- Inventory every shared outbound request-builder consumer, including WebhookKeyGet and ForwardAuth.
- Keep `SkipIfMissing` silent and keep both policies non-failing.
- Represent missing-field warnings as bounded structured evidence; never include the missing value, rendered request content, credentials, dependency detail, or unbounded attacker-controlled text.
- Route request-level observability through canonical PluginLog/access-log ownership; do not add data-plane tracing, direct logging, or a new metric.
- Preserve outbound request ordering, templates, framing, TLS, egress, retries, response caps, and both plugins' established failure policy.
- Do not modify existing integration-test files.

## Candidate scope

- `edgion-gateway/src/plugins/http/common/outbound/request.rs`
- WebhookKeyGet and ForwardAuth consumer files selected by the final ownership design
- Resource/operator documentation only if the final representation changes the documented contract

## Acceptance criteria

- `WarnIfMissing` omits the field, continues, and emits exactly the documented bounded diagnostic.
- `SkipIfMissing` remains silent; required-field failure behavior is unchanged.
- WebhookKeyGet and ForwardAuth expose the warning through the same canonical mechanism without values or dependency details.
- Focused shared-builder and consumer tests cover all three presence policies; no integration-test issue is required.
- Workspace format/check/Clippy, agent-doc, metrics, no-tracing, English-only, source-policy, and frozen-path guards pass.
