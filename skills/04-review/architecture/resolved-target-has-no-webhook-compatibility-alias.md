---
name: resolved-target-has-no-webhook-compatibility-alias
description: Use when reviewing claims that the shared outbound ResolvedTarget still has a WebhookResolvedTarget compatibility alias.
status: fixed
finding_id: gw-plugin-outbound-03
closed: 2026-08-02
---

# ResolvedTarget Has No Webhook Compatibility Alias — FIXED

## Conclusion

`ResolvedTarget` is the single canonical runtime type for shared outbound HTTP targets. The stale module comment claiming a `WebhookResolvedTarget` compatibility re-export was removed.

Fix suggestions that add a compatibility alias to make the old comment true are **not accepted**.

## Core Rationale

The shared outbound client builds and holds `ResolvedTarget`; ForwardAuth and webhook consumers use it through `ManagedOutboundClient`. No live type, alias, or re-export named `WebhookResolvedTarget` exists.

Historical design documents may mention the former name while describing earlier designs or migrations. Those references do not define the current Rust API and should not be copied into current module documentation.

## Fix Suggestions Not Accepted

- Add `pub use ResolvedTarget as WebhookResolvedTarget` — the pre-release project does not retain unrequested compatibility layers.
- Rewrite historical design records — their former-name references are valid historical context rather than current API declarations.

## Re-evaluation Triggers

Re-open this decision only if a distinct webhook-specific target type is intentionally introduced with behavior that cannot use the shared `ResolvedTarget` representation.

## Reference Cases

- `gw-plugin-outbound-03`, closed 2026-08-02.
- `edgion-gateway/src/plugins/http/common/outbound/target.rs`.
