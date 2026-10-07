---
name: request-template-policy-unification
description: Review the canonical request-template runtime, authority admission and bounded rendering, and outbound Secret provenance.
knowledge_type: regression-guards
---

# Request-template policy unification

## Conclusion and scope

Gateway request templates use one runtime segment walker and resolver. Shared
Resources admission separates trusted/request variables from resolved-Secret
permission. See the [canonical contract](../../../01-architecture/02-gateway/request-template-runtime.md).

## Rationale and evidence

`unify-gateway-request-template-runtime` identified two evaluator families, an
unrestricted authority variable set, and authority length checks after full
rendering. `render_template` now owns missing values, variable-only JSON escaping,
checked output growth, and Secret provenance. Authority restricts admission and
uses a 63-byte label budget before its existing validation and fixed-suffix compose.

The parser/types live in Resources common/request_template.rs; the runtime lives
in Gateway link_sys/template/engine.rs. The outbound request builder delegates and
retains provenance through CollectedField to HeaderValue. This replaces blanket
custom-template sensitivity with source-aware metadata, while preserving marked
header-source and cookie-substitution sensitivity as well as origin,
static-field, cookie and typed-forwarding protection. Prior blanket marking was
already safe for Secret-derived headers; this task did not establish a prior leak.

## Alternatives and limits

Missing-as-empty and missing-as-error are intentional consumer differences, not
interchangeable defaults. Empty remoteIp means unavailable; named present-empty
values remain present. ProxyRewrite valueTemplate becomes a JSON string via serde,
so pre-escaping it would double-escape. Outbound JSON escapes substitutions only.
The budget limits rendered bytes, not source allocations or allocator rounding.
No generic Secret lookup, new schema limit, tracing or metric is warranted.

## Owning resource-reference binding

Controller Secret acquisition now consumes opaque `TemplateSecretReference`
loans from actual ForwardAuth, Guardrail, AiGuard evaluator and LinkSys Webhook
request fields. Admission, legacy collection and binding share the private
OutboundRequestSpec position walker. It covers path/method, Template custom
args/headers/cookies and JSON/raw/form bodies; literal Value and origin fields
are excluded. Snapshots retain positions and ordered SecretRef descriptors,
never template literal bodies or resolved values. Literal-only and non-Secret
variable changes preserve reference authority; key, order, container, owner,
activity and placement drift invalidate it.

The canonical `parse_secret_ref` grammar remains lexical: split the last colon
and first slash and require explicit nonempty namespace, name and data key.
Only this private fixed-core-Secret adapter retains that grammar at preparation,
loan and frozen-query boundaries. Ordinary object references and actual resource
owners retain strict Kubernetes identity validation. Template target namespaces
never default to the owner namespace. Authorization and dependency registration
use the actual owner and exact same canonical GrantQuery as other bound fields.
Malformed compiled SecretRef segments report fixed DecodeError and create no
edge/read. Whole-template compiler failures contribute no Secret descriptors;
Accepted validation diagnoses those failures separately.

One managed HTTP resolver replaces the three raw-name plugin loops and clears
template/TLS materials in all four stage arrays before activity gates. Templates
remain active independently of disabled local TLS. ForwardAuth now shares local
CA/client acquisition and readonly template/TLS status with Guardrail/AiGuard.
Readonly status registers no dependencies and preserves present-empty values,
string_data fallback and non-UTF-8/missing-key errors. This implementation is one
slice of the open resource-reference contract task; complete transport/global
TLS preparation and final task verification remain separate work.

## Deep-review corrections

A second independent review found two regressions in the uncommitted migration:

- SecretRef-only provenance lost an existing sensitive HeaderValue flag when an
  outbound template copied a header set by JweDecrypt/auth code. The renderer now
  obtains the first typed header value and its flag together, preserving UTF-8
  filtering without a second lookup. Cookie substitutions remain conservatively
  sensitive. Real-adapter tests exercise typed setters, composition, outbound
  HeaderValue construction, Debug redaction and cloned requests.
- Exact capacity growth for each bounded append caused 750 growth requests for
  1,000 substitutions of 128-byte values after a 32,000-byte hint. Growth is now
  geometric, capped at the budget; tests check capacity transitions for both
  segment and JSON-character append paths, plus the existing limit boundaries.

After these corrections, 125 focused Gateway tests passed on 2026-09-20,
including `template_copies_preserve_sensitive_header_and_cookie_sources` and
the filters listed below. Workspace all-targets check and Clippy, formatting,
documentation, SSA, metrics, Pingora source, Gateway API transform and no-tracing
guards passed again. Clippy retained only the two pre-existing warnings listed
below. All 1,767 integration baseline hashes remained unchanged. Independent
runtime and security reviewers both concluded LGTM with no remaining confirmed
findings in the reviewed scope. Resources and Controller were unchanged by these
corrections; their earlier test results were not rerun.

## Verification and re-evaluation

Initial verification on 2026-09-20: 364 focused tests passed (Gateway 122,
Resources 225, Controller 17), covering admission, aggregate limits, JSON expansion,
Secret provenance/header clones, authority, timeout, CtxSet/dye, all four outbound
consumer families, and Controller Secret dependencies/rotation/status reads.
Commands used `cargo test -p <crate> --lib --offline -- <filters>`:

- Gateway: `link_sys::template`, `plugins::http::common::outbound::request`,
  `plugins::http::proxy_rewrite`, `plugins::http::ctx_set`, `plugins::runtime::dye`,
  `backends::policy::backend_traffic::authority`, `resolve_timeout`,
  `link_sys::providers::webhook`, `plugins::http::forward_auth`,
  `plugins::http::ai_guard::evaluator`, `plugins::http::guardrail::plugin`.
- Resources: `request_template`, `template`, `upstream_authority`, `link_sys::webhook`,
  `plugin_configs::ctx_set`, `plugin_configs::proxy_rewrite`, `edgion_plugins::dye`,
  `common::outbound_request`, `plugin_configs::forward_auth`,
  `plugin_configs::guardrail`, `plugin_configs::ai_guard`.
- Controller: `credential::request_secrets`, `credential::pipeline`, `test_webhook_secret_ref`.

Workspace all-targets check, formatting, documentation, SSA, metrics, Pingora source,
Gateway API transform and no-tracing guards passed. All 1,767 original integration
files retain their baseline hashes; integration scenarios were not rerun. Full
workspace Clippy passed (exit 0), with two pre-existing warnings in unchanged files:
`clippy::err_expect` in the Gateway Kafka client test at
`link_sys/providers/kafka/client.rs`, and `clippy::redundant_async_block` in
`edgion-tests/src/client/suites/edgion_plugins/dsl/grpc_error.rs`.
Independent read-only review concluded LGTM after the atomic JSON-control-escape
append correction. No commit or push was performed.
Re-evaluate when adding a variable, template consumer, Secret sink, output encoding,
or changing PluginSession missing-value semantics or resource readiness.

## Resolution references

- `unify-gateway-request-template-runtime` — fixed and verified on 2026-09-20.
