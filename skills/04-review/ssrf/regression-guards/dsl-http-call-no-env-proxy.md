---
name: dsl-http-call-no-env-proxy
description: Use when reviewing the DSL `http.call` shared reqwest clients — all four runtime.rs clients are built through the canonical outbound factory with `disable_proxy: true`, preventing reqwest's default HTTP_PROXY/HTTPS_PROXY/NO_PROXY env-var pickup. Making the proxy configurable was evaluated and declined (no demand); findings that suggest "add env/configurable proxy support" should be closed per the decision recorded below.
knowledge_type: regression-guards
---

# DSL `http.call` shared clients disable env proxy — FIXED (was FB-01)

## Conclusion

The four process-global clients in `edgion-gateway/src/plugins/http/dsl/runtime.rs`
are selected by `(block_private_ips, follow_redirects)` and built through
`edgion_common::link_sys_core::http_client::build_outbound_client` with
`OutboundClientConfig { disable_proxy: true, .. }`. The shared factory owns the
translation to reqwest's builder API, so all variants deliberately ignore
`HTTP_PROXY`, `HTTPS_PROXY`, and `NO_PROXY`. This matches the project's other internal
HTTP clients:

- `edgion-gateway/src/plugins/http/all_endpoint_status/transport.rs`
- `edgion-gateway/src/backends/health/check/probes.rs`

## Why proxy env vars are disabled

In Kubernetes clusters, admission webhooks commonly inject `HTTP_PROXY` / `HTTPS_PROXY`
into every Pod's environment so that user workloads honor the corporate egress proxy.
Edgion's internal HTTP clients are gateway control / observability paths (health probes,
endpoint status, DSL outbound), not user workloads — they must connect to the targets
the operator configured, not through whatever proxy the cluster injects. Silently
honoring the env vars would:

- leak request URLs and headers (potentially including DSL-script-injected auth tokens)
  to the proxy operator,
- expose the gateway to traffic redirection if `HTTPS_PROXY` is misconfigured or
  attacker-influenced,
- diverge from the behavior of the two peer internal clients without justification.

## Configurable proxy — evaluated and declined (WON'T DO)

During FB-01 the question arose: should DSL outbound proxy be made configurable instead
of always setting `disable_proxy: true`? This was deferred to a todo, then revisited
and declined.
The decision and its rationale are recorded here so the question is not reopened without
new information; the former `tasks/todo/dsl-http-call-configurable-proxy.md` has been
deleted and this section is now the canonical record.

**Decision: keep environment-proxy inheritance disabled on all four clients. Do not
add proxy configuration until a real deployment story requires it.**

Why declined:

- **No demand.** Nobody has reported needing DSL scripts to reach an external API
  through a corporate proxy. Building config ahead of a concrete story is churn, and the
  current factory policy already matches every other internal client in the
  project (health probes, all_endpoint_status, webhook, common/http_client).
- **Nothing is broken.** FB-01 closed the URL/header leak; there is no defect to fix.
- **Envoy cross-check reinforces "don't".** Envoy never reads env-var proxies and never
  lets a script hold a raw outbound URL — `httpCall`/`dispatch_http_call` take a *named
  cluster*, and "go through a corporate proxy" is expressed as a property of that cluster
  (or via the dynamic_forward_proxy filter), invisible to the script. So an env-var
  opt-in (design Option 1 below) runs *against* the prevailing design philosophy, and a
  per-target proxy (Envoy's model) maps to per-`http.call` overrides, not a per-script
  field — a shape we should only commit to with a real requirement in hand.

Design space, if a real story later appears (in rough order of preference):

1. **Explicit per-script proxy** — add `proxy: Option<ProxyConfig>` to `DslConfig`
   (HTTP/HTTPS endpoint, no-proxy list, optional basic auth). Replace the four static
   clients, currently selected by `(block_private_ips, follow_redirects)`, with a map
   that also keys by a hash of the proxy config. Cost: schema, validation, CRD types, en+zh docs, integration test,
   secret handling for basic-auth credentials. Closest to "this one script must egress
   via proxy X". If a script hits multiple targets with mixed proxy needs, prefer a
   per-`http.call` proxy/no-proxy override argument (Envoy-style per-target) over the
   per-script field.
2. **Gateway-global proxy** — one proxy config at the gateway process level; all DSL
   plugins share it. Simpler cache, coarser. Fits "the whole gateway must honor the
   corporate proxy". Note this is *not* Envoy's model (Envoy has no global proxy switch —
   it would be "every cluster configured with the same proxy").
3. **Env-var opt-in** (`allow_proxy_env: bool` on `DslConfig`, default false) — cheapest
   to build but **not recommended**: still implicit, unauditable, and against the Envoy
   philosophy that FB-01's explicit proxy disablement aligned us with. Avoid unless an operator
   explicitly wants Pod-injected `HTTP_PROXY`/`NO_PROXY` honored.

Out of scope regardless: making the *other* internal clients (health probes,
all_endpoint_status) configurable — they are not user-script driven, so the case is
weaker.

## What to do with related findings

| Reviewer claim | Action |
|---|---|
| "Environment proxy inheritance is enabled on a DSL client" | Already fixed — verify all four variants pass `disable_proxy: true` through the shared factory. Close as false positive. |
| "DSL should support an explicit/env proxy config" | Evaluated and declined — see the "Configurable proxy" section above. Close unless the reviewer supplies a concrete deployment story; if they do, that section is the starting design. |
| "Other internal clients should also become configurable" | Out of scope. Not user-script driven; weaker case. |

## References

- FB-01 (closed) — original defect that disabled environment-proxy inheritance for DSL clients.
- Envoy Lua filter `httpCall` and dynamic_forward_proxy — the cluster-based outbound
  model referenced in the decision above.
