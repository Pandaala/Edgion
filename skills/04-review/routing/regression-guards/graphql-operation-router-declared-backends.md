---
name: graphql-operation-router-declared-backends
description: Verify GraphQL backend selection against raw same-rule declarations and immutable scoped publication.
knowledge_type: regression-guards
---

# GraphQL operation routing to declared backends

Every mapping and optional default in a source-enabled GraphqlOperationRouter
must belong to the same attaching HTTPRoute rule. Effective Service namespace/name
keys must be unique, namespace omission uses the route namespace, and delegation
is unsupported. Source-disabled entries are inert. The shared structural helper
runs against raw desired sources in Controller and current raw sources in Gateway;
conditions and Bypass cannot repair an undeclared target. Controller withdraws an
invalid rule while preserving valid siblings, so absence of a match can yield 404.

Gateway also checks effective executable composition: exactly one router/publisher
and no AI/transcoding owner. Bypassing a conflicting publisher can restore a
structurally valid attachment. Intrinsic configuration errors remain ordinary
plugin validation. Policy Block preflight precedes the generic matched-rule 500.

The canonical PluginPublisher publishes structural and effective decisions under
its existing Weak route Arc/index token and scoped runtime generation. Plugin-only,
route-only and policy updates recompute it; pinned old snapshots may finish, but
new requests cannot borrow a decision for another route generation. Active router
sources are excluded from whole-resource DSL/WAF last-known-good retention.

A bounded canonical frontend verifies the real document selector and operation
type; names are client-selected labels, not field authorization. The router borrows
selected metadata, performs an immutable exact lookup, validates the compiled
named backend with ReferenceGrant and publishes a strict internal jump. Missing,
denied or conflicting choices never silently fall back. Strict ownership is
checked before upstream I/O and at terminal request filtering.
The canonical selected-backend compatibility guard also rejects AI/transcoding
owners in that backend runtime and its pinned ExtensionRefs before plugin/origin
I/O. An incompatible owner on an unselected backend does not reject the request.

GraphqlRateLimit and routing share envelope/parser/transport bounds and four
nonwaiting process-wide permits, not SDL or Redis policy. Cost Work carries the
frontend counter forward without resetting. No AST/body cache or response buffer
is added. Auth and rewrites precede both plugins; operators keep later request
mutation out of the admitted chain. Quota debit is not refunded on routing failure.

Evidence: [current architecture](../../../01-architecture/02-gateway/00-overview.md#httproute-graphql-operation-routing),
[structural validator](../../../../edgion-resources/src/resources/edgion_plugins/plugin_configs/graphql_operation_router.rs),
[publication](../../../../edgion-gateway/src/plugins/publication/publisher.rs),
[runtime](../../../../edgion-gateway/src/plugins/http/graphql_operation_router/plugin.rs),
[focused tests](../../../../edgion-gateway/tests/unit/plugins/publication/mod_test.rs),
and [shared integration](../../../../edgion-tests/src/client/suites/edgion_plugins/graphql_operation_router/mod.rs).
Re-evaluate changes to parser ownership, masks, delegation, target lookup, tokens,
last-known-good retention, or strict jump guards against these tests and the
[shared cost budget guard](../../rate-limit/regression-guards/graphql-query-cost-budgets.md).
