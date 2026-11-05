---
name: delegated-extensionref-remains-child-local
description: Use when reviewing HTTPRoute/GRPCRoute delegation findings about which Namespace resolves an ExtensionRef filter at each level of a delegated chain.
knowledge_type: regression-guards
---

# Delegated ExtensionRefs remain local to the declaring route

## Conclusion and scope

An ExtensionRef is a local object reference. Under the cascading-match delegation
model, every level of a matched chain (root, intermediate parents, leaf) is a
separately published route, and each level executes its own ExtensionRefs against
EdgionPlugins in **its own route's namespace**. A parent filter resolves in the
parent namespace; a child filter resolves in the child namespace. The chain runs
parent first, then child, in every stage. Locality controls lookup only; it does
not change execution order or permit an explicit cross-Namespace plugin reference.

Scope: HTTPRoute and GRPCRoute delegation on the Controller (dependency
registration, validation) and Gateway (publication, per-request resolution).

## Rationale and evidence

- No rule is copied between routes any more, so there is no locality to preserve
  in transport. The former `LocalObjectReference` namespace stamp was removed
  together with Controller flattening (Task 12 of the redesign); an injected stamp
  is now an ignored unknown field.
- Controller: each route registers plugin dependencies only for its own filters
  under its own owner (`handlers/http_route.rs` / `grpc_route.rs`
  `register_plugin_refs`, iterating `references.authored()`).
- Gateway publication: EdgionPlugins referenced by delegation children (routes
  with `resolved_delegation_keys`) are compiled into **every** Gateway plugin scope
  in the process, each with that Gateway's config, so per-Gateway allow/deny policy
  and AI/GraphQL admission still apply (design §6.4).
- Gateway request path: the leaf keeps the ordinary `resolved_route_ext_ref`
  array. Each ancestor gets its own per-level array in
  `DelegatedMatch::ext_refs`, installed only on the delegated path
  (`pg_request_filter.rs::install_delegated_ext_refs`) after the root level has
  captured the matched Gateway's plugin scope. Ordinary routes are unchanged.
- Each level's plugins and templates (`${matchedHttpRoute:...}`, labels,
  annotations, backend-name checks) see their own level's route and rule, so a
  tenant leaf cannot change parent-owned plugin settings (design §6.5).

## Alternatives and limits

- Fall back to the parent Namespace when the child-local object is absent:
  rejected; a local reference would depend on object presence and could silently
  select an unintended policy.
- Resolve every filter in the root Namespace: rejected; it erases the declaring
  resource's identity and prevents child teams from owning their plugins.
- Add an explicit Namespace field to ExtensionRef: rejected; the reference is
  intentionally local, and cross-Namespace authorization would need a separately
  designed API and ReferenceGrant model.
- Limit: child per-rule plugin state is rebuilt whenever the Gateway identity set
  of the process changes (child units are recompiled for the new set), which is
  slightly broader than for ordinary routes. See the
  [cascading-match page](delegation-cascading-match-issue-010.md#accepted-limitations).

Edgion is pre-release; the earlier "look up in the flattened parent namespace"
behavior was incorrect and receives no fallback, alias or migration shim.

## Verification and re-evaluation

Historical evidence only for this page revision; the redesign tasks record their
own runs. Relevant tests: Gateway `plugins/runtime/plugin_runtime_test/ext_ref_index_tests.rs`,
`gateway_api_filters/extension_ref_test.rs`, `pg_request_filter_test/delegation_tests.rs`;
Controller `parse_ignores_authored_extension_ref_namespace_metadata` (HTTP and gRPC);
integration `HTTPRoute/Delegation` (`test_chain_filters_parent_first`, child plugins
in `edgion-deleg`).

Re-evaluate if ExtensionRef gains a namespace field, if rules are ever copied
across routes again, or if child plugin publication stops covering every
Gateway scope.

## Resolution references

- `gw-11` — 2026-08-09 (docs clarification; no compatibility shim).
- Route delegation cascading-match redesign (ISSUE-010) — per-level resolution
  replaced the flattening-era namespace stamp.
