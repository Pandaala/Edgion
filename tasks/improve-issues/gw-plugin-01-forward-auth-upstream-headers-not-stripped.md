# gw-plugin-01: ForwardAuth / ExternalAuth lets client-supplied identity headers through when the auth response omits them

- **Category**: design-error
- **Severity**: P1 (security)
- **Scope**: local
- **Location**: `edgion-gateway/src/plugins/http/forward_auth/plugin.rs:238-251` (`handle_outbound_response`), `edgion-gateway/src/plugins/runtime/plugin_runtime.rs:855-868` (native HTTPRoute `ExternalAuth` reuses `ForwardAuth::from_route`)

## Summary
On an auth success, `decision.upstreamHeaders` are copied from the auth-service response onto the
upstream request **only if the auth response contains them**. Nothing removes a value the client
sent under the same name. If the auth service returns 2xx without a configured identity header,
the client's own `X-User-ID` / `X-User-Role` reaches the backend as if the gateway had
authenticated it. The native Gateway API `ExternalAuth` filter uses the same code, so it has the
same gap.

## Current behavior (evidence)
```rust
// forward_auth/plugin.rs:241
for header_name in &self.decision.upstream_headers {
    if let Some(value) = response.headers.get(header_name.as_str()) {
        if let Ok(v) = value.to_str() {
            let _ = session.set_request_header(header_name, v);
        }
    }
}
```
- No `remove_request_header` runs for these names before or after the outbound call. `run_request`
  (`:203-260`) only reads the body, builds the outbound request, and dispatches.
- A non-UTF-8 value from the auth response is also skipped silently, so the client value survives
  in that case too.
- `ExternalAuth` (`plugin_runtime.rs:855`) builds the same `ForwardAuth` through `from_route`.
- Other identity-producing auth plugins clear their projected headers before they set them. For
  example, `HeaderCertAuth::clear_identity_headers` runs before extraction
  (`header_cert_auth/plugin.rs:336`), and JWT/OIDC use `auth_common::clear_claim_headers`.
  ForwardAuth is the outlier.

Realistic triggers:
- An auth service that returns 200 for anonymous or public paths without identity headers.
- One that sets `X-User-Role` only for privileged users.
- A rename on the auth side (`X-User-Id` → `X-Auth-User`) that silently changes which header it emits.

In each case, client-forged values become authoritative upstream.

## Industry comparison
- **Traefik ForwardAuth**: for each `authResponseHeaders` entry, the request header is deleted first
  (`req.Header.Del(key)`). The auth response value is set only if present, so the client value can
  never survive.
- **NGINX `auth_request`**: the canonical pattern
  `auth_request_set $u $upstream_http_x_user; proxy_set_header X-User $u;` always overwrites. An
  absent value becomes empty, and NGINX drops the header.
- **Envoy ext_authz** overrides only present headers. Its docs warn about this, and Istio pairs it
  with header sanitization.

## Impact
Backends that trust gateway-injected identity headers can be fully impersonated whenever the auth
service omits a configured header. The configuration looks secure because the header is listed in
`upstreamHeaders`, so operators don't expect this.

## Suggested direction
For every name in `decision.upstream_headers`, remove the client's request header unconditionally
(all values) before forwarding. Then set the value(s) from the auth response when present. Treat
an invalid or non-UTF-8 auth value as absent, which means removed. Apply this to both the
EdgionPlugins and native ExternalAuth paths, since they share the code. Optionally copy all values
(`get_all`) rather than the first. Add a regression test where the auth service returns 200
without the header and the client sends a spoofed value.

## Notes
- Checked `skills/04-review/forward-auth/` (default-header-forwarding,
  execution-time-authorization-context, deny-response-* guards). None covers success-side
  `upstreamHeaders` sanitization. `deny-response-repeated-headers-preserved.md:94` only mentions
  that the success path is separate.
- The user guide comparison table (`docs/en/user-guide/edgion-plugins/forward-auth.md:364`) equates
  `upstreamHeaders` with Traefik `authResponseHeaders`. Traefik strips; Edgion does not.
- No related item under `tasks/`.

## Verification (independent)

- **Verdict**: CONFIRMED (severity P1 unchanged)
- `edgion-gateway/src/plugins/http/forward_auth/plugin.rs:239-247`: on success, each `upstream_headers` name is set only if present (and UTF-8) in the auth response; no `remove_request_header` runs for those names anywhere in `forward_auth/` (only `authorization` under `hide_credentials`, `:249-251`).
- `plugin_runtime.rs:855-868`: native `ExternalAuth` reuses `ForwardAuth::from_route`, so it shares the gap.
- Sibling identity plugins (header_cert_auth `clear_identity_headers`, JWT/OIDC `clear_claim_headers`) clear projected headers first; `skills/04-review/forward-auth/*` has no page covering success-side upstream header sanitization.
- Industry note: Envoy ext_authz also only overrides present headers, but Traefik (which the user doc names as the equivalent) deletes first, so the documented equivalence is wrong.

## Additional evidence (wave 3)
- The admission side has the same gap. `ForwardAuthConfig::validation_error`
  (`edgion-resources/src/resources/edgion_plugins/plugin_configs/forward_auth.rs:163-195`) validates
  every `decision.clientHeaders` name: HTTP token, ≤256 bytes, and `validate_response_header_mutation`.
  It never looks at `decision.upstreamHeaders`. An invalid name is accepted, and `set_request_header`
  then fails silently at runtime, so the client's value is again the only one the backend sees.
  Protocol-owned names (`host`, `content-length`, `transfer-encoding`) are also not rejected for
  copying from the auth response.
- The fix for the strip-before-set issue should also apply the `clientHeaders` name validation to
  `upstreamHeaders`, so the Controller rejects names the Gateway can never project.

## Re-verification (feature-10-28)

- **Verdict**: STILL-VALID (severity P1 unchanged)
- `forward_auth/plugin.rs` and `plugin_configs/forward_auth.rs` are unchanged; `handle_outbound_response` is at `plugin.rs:232` and still copies configured upstream headers only when the auth response contains them.
- `plugin_runtime.rs` changed by one line at `:316` (`try_update`); the native `ExternalAuth` reuse is unchanged.
- The new `auth_common.rs` claim-header cleanup (`clear_claim_headers`) and the P1-C1 Host-destination rule in `auth-claim-header-projection.md` apply to JWT/OIDC claim projection only, not to forward_auth `upstreamHeaders`.
