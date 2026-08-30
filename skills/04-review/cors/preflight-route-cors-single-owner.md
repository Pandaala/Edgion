---
name: preflight-route-cors-single-owner
description: Use when reviewing CORS preflight ownership; a route-level CORS plugin must run only in the normal route plugin pipeline, while the global handler owns only the no-CORS fallback.
status: fixed
finding_id: change-fix-01
closed: 2026-08-02
---

# Route CORS Preflight Has One Runtime Owner — FIXED

## Conclusion

When a matched route has an `EdgionPlugins` CORS configuration, the global preflight
handler delegates to the normal route plugin pipeline without running an ad-hoc CORS
instance. The route runtime is the single owner of local preflight responses and queued
pass-through response headers.

Fix suggestions that de-duplicate every repeated `PendingHeaderOp::Append` pair are **not
accepted**.

## Core Rationale

**1. Two executions duplicate append-semantics headers**

For `preflightContinue: true`, the ad-hoc instance queued `Vary: Origin` and returned to the
normal pipeline, where the same route CORS plugin queued it again. Set-semantics CORS headers
masked the double execution, but append-semantics headers exposed it on the response.

**2. The route runtime already implements both outcomes**

The regular CORS request filter writes the local 204 response when preflight continuation is
disabled and returns `GoodNext` after queuing headers when it is enabled. A second execution
path is unnecessary and bypasses the runtime's normal ordering, conditions, and logging.

**3. Generic append semantics must remain intact**

Repeated append values are valid and sometimes required for headers such as `Set-Cookie`.
Changing the shared pending-header queue to suppress duplicates would alter unrelated plugin
behavior to hide a CORS ownership bug.

## Fix Suggestions Not Accepted

- De-duplicate all identical queued append operations — changes generic response-header semantics.
- Replace CORS `Vary` append with set semantics — can overwrite an upstream or earlier plugin's other `Vary` tokens.
- Return `Ok(true)` whenever a route CORS config exists — closes pass-through requests without an upstream response.

## Re-evaluation Triggers

Re-open this decision only if:

- Route CORS stops running in the normal request-plugin pipeline.
- The no-CORS preflight fallback is removed or assigned to another component.
- CORS response ownership moves to a precomputed route-level phase that preserves plugin conditions and ordering.

## Reference Cases

- Finding `change-fix-01` — fixed 2026-08-02.
- Finding `change-fix-03` — the former handler-result mapping became obsolete under this fix.
- Finding `gw-http-proxy-req-01` — the preceding pass-through result-mapping fix.
- `edgion-gateway/src/routes/http/proxy_http/preflight_handler.rs`.
- `edgion-gateway/src/plugins/http/cors/plugin.rs`.
