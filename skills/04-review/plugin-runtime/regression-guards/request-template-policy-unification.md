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

The resolver borrows Header, decoded Query, current/entry path, method, remote IP
and resolved Secret text until the synchronous walker appends it. Cookie, ctx,
URI and matched-route accessors retain their owned results. The final rendered
String remains independent of both session and Secret-map lifetimes. Header
sensitivity is captured before text filtering, including opaque sensitive values;
Secret substitutions remain sensitive even when absent or empty. This removes
intermediate copies without changing source selection, budgets or capabilities.

## Alternatives and limits

The Task 06 cancellation repair adds explicit `TemplateContext::Request` and
`TemplateContext::Trusted` capabilities to the same canonical walker, request
builder and outbound loop. Trusted response callbacks read only ctx, remote IP
and matched-route metadata; incoming request fields and fallback reads fail with
`UnavailableVariable`, which missing-as-empty cannot suppress. Resolved Secrets
remain a separate capability. Guardrail still selects endpoints and reads current
authority policies on every window exchange; no complete request is frozen at
header time. See [the response cancellation contract](response-callback-downstream-cancellation.md).

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

The errors-only status projection validates borrowed selected values without
populating a value map (`ARCH-108`). Data retains precedence over string_data,
including present-empty and invalid UTF-8 data. Material acquisition, including
the readonly internal HTTPRoute-auth path, still owns its required runtime map;
value collection is independent of dependency registration. SecretStore's fetched
Secret clone remains outside this change. No measured memory or latency claim
follows from removing the discarded successful-value projection.

## Owning managed-template readiness

Enabled request ForwardAuth, Guardrail and AiGuard evaluator entries and internal
HTTPRoute Edgion-auth use the same private cold map-presence predicate in
`plugins/http/common/outbound/client.rs`. Each preparation adapter supplies the
actual owning request and its resolved map. The canonical
`OutboundRequestSpec::collect_secret_ref_names` supplies exact keys; no new parser,
Secret lookup, ReferenceGrant proof or runtime segment walker is introduced.
Missing maps/keys reject preparation before TLS/client/health work. Present empty
values are valid. Intrinsic syntax validation remains separate from the collector's
best-effort discovery, and literal Value/origin fields are not scanned.

The managed gate runs before entry-cache reuse and records non-retainable
invalidity, so a new unresolved generation cannot keep old authentication or
evaluation success. Existing disabled/Block/Bypass masks and inline-only AiGuard
stay inert. Guardrail companion ownership, evaluator POST, body/retry/deadline
contracts and captured prior Arcs remain unchanged. Route preparation retains its
existing fixed error; managed runtime diagnostics do not mislabel a missing
request key as TLS material failure. Diagnostics expose neither map keys nor values.

The shared renderer still applies its existing missing-as-empty policy. Readiness
prevents these active owning consumers from reaching it with absent required
Secret keys; map presence itself proves neither authorization nor freshness.
Other outbound consumers do not acquire this gate through the shared raw client.
The appended owning-client and populated-cache/PluginStore tests cover this scope;
fresh command results belong to the current batch evidence, not the historical
verification counts below. No live watch delivery or conformance claim follows.

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

## Rendered outbound header storage (outbound-06)

The outbound collector transfers a rendered String into HeaderValue only when its
capacity equals its length. Borrowed literal/origin text and owned strings with
spare capacity retain the existing compact from_str copy. The storage decision is
local to the collected-header conversion; canonical template parsing, rendering,
capabilities, missing values and sensitivity provenance are unchanged.

The guard matters because the renderer reserves a 32-byte hint per variable:
1,000 missing substitutions plus one literal byte can leave a one-byte result in a
32,001-byte buffer. That spare buffer is still released during collection instead
of retained with a request across network awaits or retries. Eligible exact-capacity
strings transfer through locked http 1.5.0 and bytes 1.12.1 without their temporary
copy. This is workload-dependent, not a promise that every template avoids copying.

Tests compare actual canonical rendering and collection with frozen original
conversion, including 31/32/33-byte values, empty/long values and missing-variable
hints. Conversion tests cover byte validity and transferred versus compact copied
storage; collection tests retain sensitivity, redacted Debug, clones and last-valid
override behavior. Required fields, trusted contexts, Secret policy, forward_all
and retry ownership remain governed by their existing tests and owners.

Allocation totals and separate conversion/clone/drop probes are scoped evidence,
not peak RSS, full renderer timing or outbound transport throughput. Re-evaluate
if renderer capacity behavior or header-storage dependencies change. Unconditional
transfer, arbitrary spare-capacity thresholds or new template limits require their
own reviewed policy; this local guard does not authorize those changes.
