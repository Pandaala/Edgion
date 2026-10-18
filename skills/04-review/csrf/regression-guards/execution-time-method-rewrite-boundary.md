---
name: csrf-execution-time-method-rewrite-boundary
description: Review CSRF execution-time method checks, scoped rewrite ordering and GraphQL read-only transport.
knowledge_type: regression-guards
---

# CSRF execution-time protection and method rewrites

## Conclusion and scope

T03-B corrects the missing configuration guidance and distinguishing composition
coverage. CSRF checks the current method, token header and cookie when it executes.
It does not recheck after a later rewrite. Keeping that execution-point contract
was selected by the user in this fix-issue-cn run; it does not make unsafe
configured compositions safe or imply acceptance of their use.

GET -> CSRF -> rewrite POST -> cookie-authenticated write is outside the guarantee.
Place CSRF after the relevant write-producing rewrite, preserve real token sources,
and audit later operations, conditions and every selected BackendRef. Never populate
the token header from its cookie inside the chain.

POST -> rewrite GET -> CSRF skips verification and is appropriate only if the
actual final operation is read-only. POST -> CSRF -> rewrite GET still requires
a valid token. Method names alone do not prove an operation's effect.

## Rationale and evidence

The [entry/current identity contract](../../../01-architecture/02-gateway/12-edgion-plugin-dev.md#entry-and-current-request-identity)
distinguishes live get_method from frozen get_request_method.
Csrf::run_request uses get_method; ProxyRewrite and DSL use set_upstream_method.
Ordinary scoped chains execute Global -> Gateway -> Route -> selected BackendRef,
with no CSRF final reevaluation. Moving Route entries cannot move an earlier
Global check past a BackendRef rewrite.

[RestToGraphql admission](../../http-body/regression-guards/rest-to-graphql-request-adaptation.md)
uses the entry method: GET mutations reject even after a live POST rewrite.
GET queries can use POST transport without writing. This adapter-specific rule
does not create a generic CSRF seal or final-POST requirement.

Sources:
- [CSRF runtime](../../../../edgion-gateway/src/plugins/http/csrf/plugin.rs)
- [Session adapter](../../../../edgion-gateway/src/plugins/runtime/session_adapter.rs)
- [Scoped request order](../../../../edgion-gateway/src/routes/http/proxy_http/pg_request_filter.rs)
- [BackendRef execution](../../../../edgion-gateway/src/routes/http/proxy_http/pg_upstream_peer.rs)
- [Operator guide](../../../../docs/en/user-guide/http-route/filters/edgion-plugins/csrf.md)

## Alternatives and limits

No original-only getter substitution, generic request seal, static DSL guarantee,
extra token copies, traffic tracing or new metrics are introduced. Automatically
enforcing all transformations requires a separate design; late method permissions
were tracked under T06 in the former `tasks/pending/http-request-identity/03-subtasks.md`
task (absent from the current tree).

The protected-write example requires a separate read-only token bootstrap endpoint.
Authentication cookies and a genuinely state-changing origin operation are necessary
to demonstrate the dangerous composition; a rewrite alone is not an exploit.
The fixture uses a fixed synthetic session cookie and static GraphQL documents,
not a browser or external authentication service.

## Verification and re-evaluation

The Csrf suite's method_rewrite_protection_boundary case executes real plugins
and scoped resource chains, including Global/Gateway checks and BackendRef rewrites.
A dedicated listener and GatewayClass avoid modifying shared global configuration.
The bounded origin fixture retains only call/read/write counters and returns
method/path/effect, never token or credential values. Every rejected probe asserts
zero origin-call/read/write deltas; successful probes assert the generated transport
and exact read/write effect. Cases cover both POST-to-GET orders, GET-to-POST with
missing/valid tokens, unsafe later rewrites, and GraphQL entry GET query/mutation.

Independent static review passed after correcting multi-backend counter aggregation.
The local composition case is not registered in Kubernetes mode, whose fixture
resources do not include the dedicated listener. No Kubernetes claim is made.
Historical T03-B verification recorded workspace check/clippy and integration-build
blockers from merge conflict markers in plugin_runtime.rs, session_adapter.rs and
pg_upstream_response_body_filter.rs. Those conflict markers have since been resolved.
This historical note does not establish fresh integration acceptance of the
composition case. The referenced parent-task tree is absent from the current tree.
Re-evaluate if CSRF changes identity view, scoped ordering, token sources,
late-mutation policy, or RestToGraphql admission/operation semantics.

## Borrowed live Cookie reads

CSRF token verification borrows the canonical `cookie_value_ref` result from
live request headers. The production owned getter calls this same parser and
copies the selected value; the plugin no longer needs that temporary copy.
Header token precedence, safe-method handling, exact Cookie key case, first
match (including empty), embedded equals and malformed-field ordering remain.
An opaque field before a match prevents later recovery; an earlier match makes
later opaque fields irrelevant. Signature/expiry checks and fixed denial/log
messages are unchanged. Decoding and cryptographic work still allocate.

Unit fixtures now use real HeaderMaps and forbid the owned Cookie getter.
They cover padded valid tokens and multi-field credential boundaries. This does
not revalidate the historical integration composition scenario above or claim
allocation-free verification or throughput gains.

## Resolution references

- T03-B (parent issue T03): documentation and composition-test gap; production
  execution-time CSRF semantics retained by explicit user selection.
