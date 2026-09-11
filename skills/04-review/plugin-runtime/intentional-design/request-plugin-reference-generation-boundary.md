---
name: request-plugin-reference-generation-boundary
description: Review the accepted per-reference HTTP plugin pinning boundary and proposals for one request-wide PluginStore snapshot.
knowledge_type: intentional-design
---

# HTTP plugin references retain their individually resolved runtimes

## Conclusion and scope

The user declined the request-wide PluginStore snapshot improvement on 2026-09-09
after discussing generation mixing and retention costs, and explicitly requested
removal of its active task. This is a decision not to implement stronger
consistency, not a claim that generation mixing was fixed or disproved.

`PluginExecutionState` retains resolved global, Gateway, and route runtime Arcs.
`PluginAuxState.resolved_backend_ext_refs` pins each backend key's first lookup,
including a missing result. Once selected, the request's WAF owner is stable.
There is no single captured PluginStore generation shared by all these lookups.

## Rationale and evidence

- `plugins/http/plugin_store.rs`: `PluginStore.snapshot` publishes one immutable
  map atomically; `with_plugin` loads the current snapshot on each invocation.
- `routes/http/proxy_http/global_plugins_resolve.rs`: each resolver looks up keys
  separately. One compiled resource's runtime is shared as an Arc; references to
  different resources can resolve from different publications.
- `routes/http/proxy_http/pg_request_filter.rs`: HTTPRoute references are captured
  before preflight. Global references now pin in the early parsed-header callback;
  Gateway references pin there when ownership is known, otherwise immediately
  after route ownership resolves and before preflight. GRPCRoute references
  retain their post-preflight capture timing. Early misses also remain pinned.
  WAF selection uses those captured runtimes and retains its selected owner.
- `plugins/runtime/session_adapter.rs` and `ctx.rs`: backend references first
  resolve against the live store, then reuse the cached result for that key.

## Alternatives and accepted limits

Retaining the existing boundary avoids adding request-wide map ownership and
keeps later first-use lookups able to observe newer configuration. An owned map
snapshot would keep otherwise unrelated compiled resources alive until released;
capture/release timing needs design, especially for long requests.

The accepted cost is that cooperating plugin resources may observe mixed
generations during publication. Context pinning prevents changes after selection,
but does not make the inputs to selection generation-consistent. For example,
moving WAF between two already-referenced global and route bundles could leave a
request's mixed pair without a WAF even when both complete publications have one.
Producer/consumer plugins changing a shared field convention can also disagree.
These are code-derived scenarios, not reproduced incidents or demonstrated
authorization bypasses. The decision does not establish that such risks are zero.

A request-wide PluginStore snapshot would address same-store lookup mixing only;
it would not make separately published intermediate states, other resource kinds,
external credentials, or mutable dependency state transactional. The separate
[cross-kind publication decision](../../conf-sync/intentional-design/cross-kind-config-not-atomic-extensionref-torn-state.md)
does not itself justify this same-store tradeoff.

## Verification and re-evaluation

This closure changes documentation only. Source inspection verified the lookup
and pinning boundary; no deterministic generation-mixing test or integration
scenario was run for this decision. Existing backend pin tests cover repeated
lookup of one key, not coherence between keys or tiers.

Closure checks on 2026-09-09: `cargo fmt --all`,
`cargo check --workspace --all-targets`, and `cargo clippy --workspace --all-targets`
completed successfully; clippy reported existing warnings in unchanged Rust code.
The agent-doc, SSA-force, metrics-inventory, and Pingora-source-policy guards passed.

Re-evaluate if an incident or deterministic test demonstrates material mixed-policy
harm, deployments require coordinated plugin migration within one request, or a
bounded snapshot lifetime becomes available with substantially lower cost. This
decision does not forbid reporting new concrete evidence.

## Resolution references

- `http-plugin-store-request-generation-snapshot` — improvement declined by the
  user; task closed without implementation on 2026-09-09.
- `B0175` — originally deferred request-wide capture from resolver cleanup.
