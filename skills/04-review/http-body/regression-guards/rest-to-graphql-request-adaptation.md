---
name: rest-to-graphql-request-adaptation
description: Verify static GraphQL adaptation, canonical KeyGet/body replay and original-method/inspection ordering.
knowledge_type: regression-guards
---

# REST-to-GraphQL request adaptation

RestToGraphql is a request-stage adapter attached through existing HTTPRoute and
EdgionPlugins mechanisms. It introduces no path matcher, routing table, backend
publisher, GraphQL-specific KeySet target, variable cache or response owner.
Canonical KeyGet supplies local values; CtxSet/DSL prepare complex values.
Native body bindings reuse strict JSON and ProxyRewrite's compiled JSON pointers.
The cost compiler and adapter share canonical TypeRef parsing.

`RequestInfo::method` snapshots the entry Method after early RequestId handling
and request modules, before routing and ordinary rewrite plugins, and remains
frozen. RestToGraphql reads the generic borrowed
`PluginSession::get_request_method()`; missing capture fails closed without a
live fallback. Ordinary method reads use the live session. A live method rewritten from GET to
POST cannot authorize mutation, and an original physical GET body rejects.
Successful canonical GraphQL evaluation freezes mutation through the existing
RequestBodyState owner, which propagates to future bodyless storage. Every
adapter must publish through this owner; a late adapter therefore fails closed.
No GraphQL execution flags are added to EdgionHttpContext. Multiple adapters
before final inspection are allowed. Failed/skipped consumers do not freeze.
The current frozen-owner publication refusal is `BodyReadError::NotBuffered`;
`GenerationConflict` denotes a stale logical generation in the storage owner.
The bodyless GET regression verifies that a future replay owner is installed,
retains length zero and empty bytes, and refuses replacement after inspection.
It therefore distinguishes an enforced freeze from a missing-cache refusal.
The freeze also applies to subsequent DSL/ProxyRewrite/Wasm body mutations,
enforcing the existing documented rewrites-before-inspection contract. It is
body-only; query/URI/method/header mutations retain trusted-chain restrictions.

Query KeyGet observes the entry query. Strict preflight checks encoded length,
percent/UTF-8 validity and decoded duplicates before information-losing lookup.
Mapped header duplicates/non-UTF-8 values reject before KeyGet reads. Every
nonempty logical body requires supported JSON media/identity encoding and strict
parsing, even when ignored. No body-backed KeyGet I/O variant is introduced.

Missing nullable/defaulted variables are omitted, preserving defaults. Explicit
null remains distinct and rejects non-null wrappers. Builtin scalar and bounded
list types are checked; custom SDL/default-literal semantics remain the upstream's
responsibility. All selected variable wrappers enforce the same eight-level bound,
including optional/defaulted variables without mappings. Per-value, binding,
input and final-envelope caps are independent; the complete generated envelope
is checked by the canonical frontend before mutation. CappedWriter prevents an
oversized serialized output allocation. Four shared nonwaiting permits bound
construction; parsed owners and permits are dropped before publication.

Publish only through canonical BodyStorage/replay, method/URI/header mutations.
Consume REST query parameters. Upstream framing owns Content-Length and removes
Transfer-Encoding on each attempt. Any publication failure terminates before
upstream work; rollback is not a reason to continue with a partial request.
Use fixed GraphQL errors and static log decisions, never reflect query/variables
or runtime error text. Main upstream responses continue streaming.

Sources and focused tests:

- [Shared config and binding compiler](../../../../edgion-resources/src/resources/edgion_plugins/plugin_configs/rest_to_graphql.rs)
- [Adapter runtime](../../../../edgion-gateway/src/plugins/http/rest_to_graphql/mod.rs)
- [Resources tests](../../../../edgion-resources/tests/unit/resources/edgion_plugins/plugin_configs/rest_to_graphql_test.rs)
- [Gateway tests](../../../../edgion-gateway/tests/unit/plugins/http/rest_to_graphql/mod_test.rs)
- [Shared static/dynamic probes](../../../../edgion-tests/src/client/suites/edgion_plugins/rest_to_graphql/mod.rs)
- [Current architecture contract](../../../01-architecture/02-gateway/00-overview.md#httproute-rest-to-graphql-adaptation)

Re-evaluate source timing, accessor contracts, method snapshots, canonical body-freeze ownership,
custom input validation, bound changes, body publication/framing and hot reload
against fresh tests. The former implementation task is no longer present; use
the linked architecture, sources and tests for current evidence. This page does
not establish a successful live run.

## Scalar-bound traversal ownership

The shared binding validator always profiles input/output values and enforces
node, depth and text-byte limits first. A scalar root then skips the separate
array-length traversal: it cannot contain an array, so allocating that worklist
would perform no useful check. Array/object roots still traverse every nested
container and reject arrays longer than 32. Profiling failure remains Invalid;
adapter-specific cap violations remain TooLarge, in the original order.
Direct tests cover scalar byte limits, nested arrays and aggregate depth/node
boundaries. Binding, JSON conversion and envelope/publication ownership are
unchanged. Profiling itself still allocates; this is not allocation-free binding
or a throughput claim.

## Binding input ownership

The Gateway passes existing KeyGet-owned strings as owned JSON values into the
shared binder, while native body-pointer values remain borrowed. `bind` accepts
`Cow<Value>` and profiles the input before any clone or conversion. Plain text
can move into the final variable without duplicating its String allocation;
borrowed body values clone only after profiling. JSON/numeric/list conversion,
final value bounds and type validation retain their existing order and errors.
Header duplicate/opaque/byte preflight and final envelope validation remain.

Tests compare owned and borrowed inputs across types and caps, check the moved
text allocation identity, and verify native body input remains independent of
the bound result. Input acquisition, parsing, final variable ownership and output
serialization still allocate; no whole-binding allocation-free claim follows.
