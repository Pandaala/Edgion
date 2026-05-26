---
name: dsl-http-call-no-env-proxy
description: Use when reviewing the DSL `http.call` shared reqwest client — the two static clients in runtime.rs are intentionally constructed with `.no_proxy()` to disable reqwest's default HTTP_PROXY/HTTPS_PROXY/NO_PROXY env-var pickup. Findings that suggest "add env proxy support" should be redirected to tasks/todo/dsl-http-call-configurable-proxy.md.
---

# DSL `http.call` shared client disables env proxy — FIXED (was FB-01)

## Conclusion

Both `DSL_HTTP_CLIENT` and `DSL_HTTP_CLIENT_GUARDED` in
`src/core/gateway/plugins/http/dsl/runtime.rs` call `.no_proxy()` on the
`reqwest::Client::builder()`. This is the intended end state — reqwest's default
behavior of reading `HTTP_PROXY`, `HTTPS_PROXY`, and `NO_PROXY` from the process
environment is deliberately disabled. This matches the project's other internal reqwest
clients:

- `src/core/gateway/plugins/http/all_endpoint_status/plugin.rs:121`
- `src/core/gateway/backends/health/check/probes.rs:42`

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

## What to do with related findings

| Reviewer claim | Action |
|---|---|
| "`.no_proxy()` is missing on the DSL client" | Already fixed — verify by re-reading `runtime.rs`. Close as false positive. |
| "DSL should support an explicit proxy URL config" | Redirect to `tasks/todo/dsl-http-call-configurable-proxy.md`. Not implemented because no current user demand; design space is documented there. |
| "Other internal clients should also become configurable" | Out of scope. Not user-script driven; weaker case. Same todo file notes this. |

## References

- FB-01 (closed) — original defect that introduced `.no_proxy()` on both clients.
- `tasks/todo/dsl-http-call-configurable-proxy.md` — design space for a future
  configurable-proxy feature, if a real deployment story requires it.
