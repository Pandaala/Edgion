---
name: request-template-runtime
description: Canonical request-template compilation, admission, rendering policies, output budgets, and Secret provenance.
---

# Request-template runtime

## Ownership

`edgion-resources/src/resources/common/request_template.rs` owns the parser,
compiled segments, Secret-reference helpers, and typed admission policies.
`edgion-gateway/src/link_sys/template/engine.rs` owns the only runtime segment
walker and variable resolver. Compile during configuration preparation; substituted
`${...}` text remains literal data. The separate ProxyRewrite `$uri` language is
outside this mechanism. Its final named-path substitution preserves original token
spans and escaped injected dollars; see the [ProxyRewrite boundary guard](../../04-review/plugin-runtime/regression-guards/proxy-rewrite-path-token-boundaries.md).

Admission combines `VariablePolicy::{Request, Trusted}` with
`SecretPolicy::{Deny, AllowResolved}`. Trusted variables are ctx, remoteIp and
matched HTTP/gRPC route metadata; Secret permission is independent. Authority
and timeout deny Secrets and client-controlled variables. Existing restricted
outbound credential surfaces can allow resolved Secrets. Trust in ctx depends
on its producer; remoteIp retains the existing proxy trust boundary.

## Consumer contracts

| Consumer | Missing value | Escaping | Rendered byte budget | Secret source |
|---|---|---|---|---|
| CtxSet, dye | Empty | None | None | Denied |
| Upstream authority label | Empty | None | 63 | Denied |
| ProxyRewrite JSON valueTemplate | Error (400) | None; serde serializes the string | Existing JSON body limit (413) | Denied |
| Outbound path/method/fields/raw/form | Empty | None; field sinks encode | None | Controller-carried map |
| Outbound JSON body | Empty | Variable bytes only | None | Controller-carried map |
| Outbound timeout | Empty, then configured fallback | None | None | Denied |

Named Some("") values are present. Empty remoteIp means unavailable and follows
the missing-value policy. Preserve current versus entry-time path/URI accessors.
Required outbound origin fields retain their separate PresencePolicy.

Authority renders only the dynamic label, checks before appending, then validates
and normalizes it before attaching the fixed suffix. Empty yields RenderEmpty;
oversized/invalid labels yield InvalidLabel. Service anchoring and pinned dialing
remain unchanged. Timeout retains its floor, upper clamp, fallback, and one fallback
metric per logical call. No new failure-policy shape or metric is introduced.

The checked output writer counts aggregate UTF-8 bytes, including JSON escape
expansion, before each append. Literal segments are never escaped. Optional limits
bound logical output bytes, not source Strings or allocator rounding. Do not add
operator-configured limits simply to make all policies uniform.
Bounded capacity grows geometrically, clamped to the budget, rather than requesting
one allocation per segment or escaped character.

## Secret ownership and sinks

The renderer receives `SecretCapability::Denied` or a borrowed
`SecretCapability::Resolved(Option<&ResolvedSecrets>)`. It never looks up Secrets
through PluginSession or a global store. An absent allowed map follows missing
policy; it does not override resource readiness or controller dependency rules.
Preserve Secret rotation, ReferenceGrant checks, admin redaction and schema exclusion.

`RenderedTemplate` carries the String and sensitivity together, with redacted
Debug. Allowed Secret substitutions mark the result even when absent or empty.
Header substitutions preserve the source HeaderValue's sensitive flag through a
single typed lookup; cookie substitutions remain conservatively sensitive because
the cookie accessor does not expose per-cookie provenance. Missing/UTF-8 filtering
and first-header-value semantics are unchanged.
Outbound custom-field assembly retains provenance until HeaderValue construction;
Secret-derived values are sensitive before insertion/cloning/transport, regardless
of header name. Templates without Secrets, marked source headers or cookie values
are not marked solely for being templates. Typed forwarded headers preserve their flags; explicit
origin headers, static custom fields, and reconstructed cookies retain conservative
protection. This is not a general PII or context-variable taint classifier, but
known sensitivity must never be discarded at a typed header source.

Webhook, ForwardAuth, Guardrail and AI Guard share CompiledRequest/outbound_call.
Changes must verify all four consumers and header-only collection. Render errors
and request-build diagnostics never include expanded values or destinations.

## Extension and verification

New consumers select explicit admission, missing, escape, capability and budget
policies. Do not add a consumer-local variable switch or segment walk. Thin facades
may map results into existing consumer outcomes.
Test variable present/missing/empty states, 0/exact/+1 and aggregate byte budgets,
UTF-8/JSON expansion, literal injection, Secret capability/provenance, and actual
header sinks. Preserve existing integration tests when performing this migration.
