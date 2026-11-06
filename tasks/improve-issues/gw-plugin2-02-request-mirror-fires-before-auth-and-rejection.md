# gw-plugin2-02: RequestMirror sends the shadow request at its filter position, so requests later rejected by auth / WAF / rate limit are still mirrored, carrying unauthenticated client identity headers

- **Category**: design-error
- **Severity**: P2 (security-relevant; downgraded from P1 in independent verification)
- **Scope**: local
- **Location**:
  - `edgion-gateway/src/plugins/runtime/plugin_runtime.rs:784-886` (route filters compiled into `request_plugins` in spec order; `RequestMirror` at `:834`, `ExternalAuth` at `:855`, `ExtensionRef` at `:870`)
  - `edgion-gateway/src/plugins/runtime/plugin_runtime.rs:3048-3053` (`add_request_filter` appends)
  - `edgion-gateway/src/plugins/http/request_mirror/plugin.rs:291-525` (`run_request` spawns the writer immediately; bodyless and committed-body cases are completed at `:498-502`)
  - `edgion-gateway/src/plugins/http/request_mirror/plugin.rs:253-279` (`build_headers`: copies every client header, sets `Host` to the endpoint `ip:port`)

## Summary
RequestMirror is an ordinary request-stage filter. When it runs, it immediately spawns the shadow
request with all client headers. For bodyless requests and already-committed bodies, it completes
the request at once (`cancellation.complete()`). Any filter or plugin that runs later and rejects
the request does not stop the mirror. This includes JwtAuth, KeyAuth, ExternalAuth, WAF,
RateLimit, RequestRestriction and CSRF. Gateway API leaves filter order to the author, and Edgion
runs filters in declaration order. So a rule written `filters: [RequestMirror, ExtensionRef(auth)]`
mirrors unauthenticated, WAF-blocked and rate-limited traffic to the shadow service. The shadow copy
also carries the **client-supplied** values of identity headers such as `X-Consumer-Username` and
`X-User-ID`, because the auth plugins that would overwrite them have not run yet.

## Current behavior (evidence)
- `plugin_runtime.rs:784-886`: each `rules[].filters` entry is compiled in order and pushed with
  `add_request_filter` (`:3052` `self.request_plugins.push`). The same holds for
  `EdgionPlugins.requestPlugins`, where a `RequestMirror` entry is ordered with auth plugins
  (`:1388`). There is no special late stage for mirroring.
- `request_mirror/plugin.rs:383`: `tokio::spawn` starts the reqwest send right away. At `:498-502`,
  when the body is already committed or the request has no body, the body channel is closed and
  `cancellation.complete()` is called. From then on, nothing in the main request path can cancel
  the mirror.
- `build_headers` (`:253-279`) forwards every non-hop-by-hop header. That includes
  `Authorization`, cookies and any identity header the client chose to send. It replaces `Host`
  with the resolved endpoint `ip:port` (`:274-276`).
- The ordering doc (`docs/en/user-guide/http-route/filters/plugin-composition.md:54-70`) states the
  declaration order and lists RequestMirror as "Request (non-blocking)". It does not warn that later
  rejections do not suppress the mirror.

## Industry comparison
- **Envoy / Envoy Gateway / Istio**: mirroring is a **router** feature (`request_mirror_policies`).
  It runs only after the HTTP filter chain (ext_authz, jwt_authn, rbac, ratelimit, WAF) has accepted
  the request. A request rejected by any filter is never shadowed. Envoy also keeps the original
  authority and appends `-shadow` to it, so virtual-host-based shadow services still route.
- **NGINX `mirror`**: runs in the precontent phase, after access-phase checks (`auth_request`,
  `allow/deny`, `limit_req` in preaccess), so rejected requests are not mirrored.
- **Kong / APISIX `proxy-mirror`**: runs in the access phase at a priority below the auth plugins,
  so auth always precedes the mirror.

## Impact
- **Auth bypass on the shadow environment.** Shadow services usually trust the gateway: they read
  injected identity headers and do not re-authenticate. An attacker who sends
  `X-Consumer-Username: admin` with no credential gets 401 from the primary path. The shadow
  service still receives the request with the forged header and executes it, writes included.
- WAF-blocked payloads (SQLi/RCE probes) and rate-limited floods reach the shadow backend
  unfiltered. Rate limiting then fails to protect the shadow tier from amplification.
- The `Host` value is replaced with a pod IP, so shadow deployments behind host-based routing (or
  with host-dependent logic) receive requests they cannot route correctly.

## Suggested direction
1. Make mirroring a **post-filter dispatch**. At compile time, `RequestMirror` should only record a
   "mirror intent" in ctx (target, sampling decision, snapshot handle). Dispatch it once the request
   plugin chain has finished with `GoodNext` and the request is about to be proxied, for example at
   upstream peer selection. A terminated request (`ErrResponse` / `ErrTerminateRequest`) drops the
   intent. This matches Envoy's router-level semantics, regardless of spec order.
2. Build the mirrored headers from the **post-filter** request header map, so auth-injected identity
   and stripped credentials (`hideCredentials`) are what the shadow sees.
3. Preserve the original `Host`/`:authority` and optionally add a `-shadow` suffix, as Envoy does.
   Do not overwrite it with the endpoint address.
4. Until then, add a validation warning when a `RequestMirror` precedes an auth or limit filter in
   the same rule, and document the behavior.

## Notes
- Checked `skills/04-review/request-mirror/` (separate HTTP client accepted tradeoff,
  ReferenceGrant, budgets, committed-body snapshot, unit tests). None of them covers dispatch timing
  relative to later rejecting filters, or the Host rewrite. The accepted tradeoff
  `mirror-uses-separate-http-client-bypasses-lb-tls-policy.md` concerns LB/TLS policy, not request
  semantics.
- `skills/04-review/security/intentional-design/auth-identity-headers-overwrite-written-names.md`
  accepts last-write-wins identity headers *on the primary path*. This finding is different: the
  mirror snapshots headers before any auth plugin has written them.
- No related task under `tasks/`.

## Verification (independent)

- **Verdict**: DOWNGRADED (P1 → P2)
- Claims hold: `edgion-gateway/src/plugins/http/request_mirror/plugin.rs:383` spawns the writer immediately, and `:492-503` calls `cancellation.complete()` for bodyless/committed-body requests, so later rejections cannot cancel it; `build_headers` (`:253-279`) copies all client headers and replaces `Host` with the endpoint target.
- `docs/en/user-guide/http-route/filters/plugin-composition.md:37-40,56-59`: execution is global → route rule → backendRef, and declaration order within a source. Gateway API asks implementations to honor filter order where possible, so the exposure requires a route author to place `RequestMirror` before an auth/WAF/limit filter in the same rule (or put auth on backendRef filters). Gateway-level/global auth already runs first.
- Why lower: a real footgun with no warning and a real divergence from Envoy's router-level mirroring, but it is conditional on rule ordering and a shadow tier that trusts gateway identity headers, not a default bypass. The `Host` rewrite to `ip:port` is a separate valid compatibility point.

## Re-verification (feature-10-28)

- **Verdict**: STILL-VALID (severity P2 unchanged)
- `plugins/http/request_mirror/plugin.rs` has no diff; `plugin_runtime.rs` route-filter compilation order unchanged (one-line rename elsewhere).
