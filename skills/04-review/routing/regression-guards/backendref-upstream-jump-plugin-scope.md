---
name: backendref-upstream-jump-plugin-scope
description: Use when reviewing upstream-selection request plugins attached through an HTTPRoute or GRPCRoute backendRef ExtensionRef.
knowledge_type: regression-guards
---

# BackendRef upstream-jump plugin scope

## Conclusion and scope

An enabled `DirectEndpoint`, `DynamicInternalUpstream`, `DynamicExternalUpstream`,
`RegionRoute`, or `Canary` request plugin cannot run through an HTTPRoute or
GRPCRoute `backendRefs[].filters.ExtensionRef`. Those plugins publish an upstream
jump, but backendRef request filters run only after a backend has been selected.
The Controller marks the affected rule invalid; the Gateway independently
rejects the referenced plugin runtime before executing it. Disabled entries do
not trigger this restriction. Rule, Gateway, and global scopes retain their
existing upstream-selection behavior.

When a Route has a valid sibling rule, the offending rule is dropped with
`PartiallyInvalid=True` and the attachment remains eligible. Only an attachment
with no surviving rule is rejected. The pre-existing backend-scoped WAF and
RequestId ownership errors retain their separate route-wide status behavior.

## Root cause and correction

The HTTP upstream path checked `ctx.jump` before `select_http_backend`, then
ran the selected backend's request plugins. A backend-scoped publisher could
write a jump after the dispatch decision. The first attempt continued with
the original backend; a later HTTP retry could encounter a retained Direct or
External jump at the earlier check. The gRPC selector likewise ran backend
plugins after selection and had no jump-consumption branch.

`EdgionPlugin::publishes_upstream_jump` and
`is_upstream_jump_plugin_type` provide one classification for Controller and
Gateway. `backend_request_scope_extension_ref_error` examines active source
entries during HTTPRoute and GRPCRoute validation. `PluginRuntime` records the
classification when compiling enabled request filters, including shared entry
bundles. `ExtensionRefFilter` checks it on the backend-scoped path before
request execution; its silent resolution path uses the same boundary.

This restriction concerns only the five in-tree plugins that call
`PluginSession::set_upstream_jump`. It does not prohibit ordinary backendRef
request or response filters, and it does not infer that arbitrary DSL or Wasm
behavior publishes a jump. A new jump publisher must join the shared
classification and the scope tests.

## Verification and re-evaluation

The focused Controller test covers each of the five types as enabled and
disabled. The Gateway test checks that a compiled jump publisher is rejected
through a backend-scoped ExtensionRef. HTTPRoute and GRPCRoute two-rule status
tests guard sibling preservation. Live hot-reload coverage remains tracked
in `tasks/todo/backendref-upstream-jump-scope-dynamic-coverage.md`. Re-evaluate if backendRef filters move
before backend selection, a bounded late-selection lifecycle is designed, or
another plugin gains the jump setter.

## Resolution references

- `define-backendref-upstream-selection-plugin-semantics` — reject upstream-jump plugins at backendRef scope.
