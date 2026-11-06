# gw-core-02: Missing Gateway-level and EGC-global HTTP plugin references are silently dropped (fail-open)

- **Category**: design-error
- **Severity**: P1 (security)
- **Scope**: local
- **Location**:
  - `edgion-gateway/src/routes/http/proxy_http/global_plugins_resolve.rs:24-26,51-58` (`resolve_global_plugins`, `resolve_gateway_plugins`)
  - `edgion-gateway/src/routes/http/proxy_http/pg_request_filter.rs:244-249` and `pg_early_request_filter.rs:76-80` (install sites)
  - `edgion-gateway/src/plugins/publication/publisher.rs:550` (`PluginTarget::Missing => continue`)
  - Contrast: `edgion-gateway/src/plugins/runtime/gateway_api_filters/extension_ref.rs:143-150` (ExtensionRef → 500) and `edgion-gateway/src/routes/tcp/edgion_tcp.rs:369-371` (stream plugins → Deny)
  - Controller: `edgion-controller/.../handlers/gateway.rs:2823` (`validate_gateway_plugin_ref` only sets `ResolvedRefs=False`)

## Summary
HTTP plugins can attach through three references: Gateway-wide `edgion.io/edgion-plugins` annotations,
EdgionGatewayConfig global plugin lists, and rule-level ExtensionRefs. If a Gateway annotation or EGC global
list names an `EdgionPlugins` object that does not exist in the published scope, the request path drops
that reference and serves the request without the plugin. This happens when the object was deleted,
renamed, mistyped, pruned by GitOps before the Gateway was updated, or not yet delivered. Rule-level
ExtensionRefs fail closed with a 500 in the same situation, and connection-level stream plugins fail closed
with a deny. The broadest attachment points are the ones used for gateway-wide authentication, IP
allow-lists, WAF, or rate limits. On those points, the security plugin can disappear without any data-plane
signal.

## Current behavior (evidence)
- Publication: `publisher.rs:550` skips `PluginTarget::Missing` keys when compiling the class scope, so
  `PluginScope::http_runtime(key)` returns `None`. Ambiguous targets, by contrast, get an explicit
  `CompiledPlugins::ambiguous_reference()` placeholder that fails closed.
- Request path: `resolve_gateway_plugins` (`global_plugins_resolve.rs:51-58`) does
  `if let Some(rt) = store.http_runtime(key) { out.push(rt); }`, and `resolve_global_plugins` reuses it. The
  module doc (`global_plugins_resolve.rs:7`) states: "Dangling references are silently dropped
  (global/gateway refs) or kept as a `None` placeholder (ext refs)". The only stated reason is avoiding
  per-request logging. That justifies not logging, not proceeding without the plugin.
- `pg_request_filter.rs:244-249` installs the reduced list, and the request proceeds through route matching,
  upstream selection and proxying with no Gateway-level plugin.
- Inconsistent siblings:
  - ExtensionRef with a missing target: `extension_ref.rs:143-150` pushes `"extension ref not found"` and the
    callers map that to a 500. This is documented as the fail-closed contract in
    `skills/04-review/conf-sync/intentional-design/cross-kind-config-not-atomic-extensionref-torn-state.md`,
    whose whole argument rests on "the window fails closed, it does not let a request through with security
    plugins bypassed".
  - Stream plugins with a missing resource: `edgion_tcp.rs:369-371` returns
    `StreamPluginCheck::Deny("stream plugin resource not found")`.
- Control plane: `validate_gateway_plugin_ref` only sets the Gateway's `ResolvedRefs=False`. The Gateway
  stays `Accepted`/`Programmed`, and nothing on the data plane withholds traffic.

## Industry comparison
- **Envoy / Istio**: an HTTP filter whose typed config references an unavailable resource makes the
  listener update NACK, and the last good config, which still contains the filter, stays active. Envoy
  Gateway `SecurityPolicy` / `ext_authz` with an unresolvable reference renders a route that returns 500
  (fail closed).
- **Kong**: a global plugin is a row in the plugins table. It cannot dangle; deleting it is an explicit
  removal, never an accidental bypass.
- **APISIX**: a `global_rule` referencing a missing `plugin_config` fails the route with 503 rather than
  running without it.

## Impact
- Security regression on routine operations:
  - `kubectl delete -f dir/` or a GitOps prune that removes the `EdgionPlugins` before the Gateway is
    updated.
  - A namespace or name typo in the annotation.
  - Moving the plugin object between namespaces.

  Each of these turns a gateway-wide JWT/OIDC/IP-restriction/WAF policy off for every route on that
  Gateway, with no access-log or metric signal. Operators only find out from a status condition they may
  not watch.
- The behavior contradicts the project's failure-policy rule ("auth = fixed fail-close") and the
  accepted-tradeoff argument cited above. That argument holds for ExtensionRef but is false for these two
  attachment points.

## Suggested direction
- Make a missing global or Gateway-level target fail closed, exactly like `Ambiguous`. In `publisher.rs`,
  compile `Missing` into a placeholder runtime (for example a `CompiledPlugins::missing_reference()` that
  terminates with 500 and records a bounded `EdgionErrorCode`). Then `resolve_gateway_plugins` can push
  every key unconditionally. This keeps the hot path log-free and consistent with ExtensionRef and stream
  plugins.
- Optionally add an explicit, per-reference opt-out (for example `optional: true` on the EGC
  `PluginReference`) if operators need "apply if present" semantics. The default must stay fail-closed.
- Surface the condition on the data plane, for example with a bounded diagnostic code in the access log, so
  the 500s are attributable.

## Notes
- Checked `skills/04-review/conf-sync/intentional-design/cross-kind-config-not-atomic-extensionref-torn-state.md`.
  It explicitly scopes its fail-closed argument to ExtensionRef and does not cover Gateway annotation or EGC
  global references. This finding is new evidence that the "no security bypass" premise does not hold for
  those paths.
- Also checked `skills/04-review/routing/regression-guards/gateway-api-extensionref-validation-fails-closed.md`
  and `observability/mandatory-design/global-plugins-resolve-no-tracing.md`. The latter forbids logging
  here, which is unaffected by this fix.
- No `tasks/` item tracks this.

## Related findings

- ctrl-proc2-02 (EdgionGatewayConfig reports Accepted=True while global plugins / IP lists are missing — the controller-side status half of this fail-open).

## Verification (independent)

- **Verdict**: CONFIRMED (severity P1 unchanged)
- `edgion-gateway/src/plugins/publication/publisher.rs:550`: `PluginTarget::Missing => continue`, while `Ambiguous` gets a fail-closed `ambiguous_reference()` placeholder.
- `edgion-gateway/src/routes/http/proxy_http/global_plugins_resolve.rs:51-58`: `resolve_gateway_plugins` pushes only `Some` lookups; `resolve_global_plugins` delegates to it, so missing Gateway/EGC-global targets are dropped and the request proceeds.
- `skills/04-review/observability/mandatory-design/global-plugins-resolve-no-tracing.md` only forbids per-request logging; it describes "the plugin simply does not run" as the observable signal but never evaluates the fail-open security semantics. `conf-sync/intentional-design/cross-kind-config-not-atomic-extensionref-torn-state.md` scopes its fail-closed argument to ExtensionRef. No accepted design covers this.

## Re-verification (feature-10-28)

- **Verdict**: STILL-VALID (severity P1 unchanged)
- `edgion-gateway/src/plugins/publication/publisher.rs:550`: `PluginTarget::Missing => continue` unchanged (no diff to the file).
- `edgion-gateway/src/routes/http/proxy_http/global_plugins_resolve.rs:51-58`: `resolve_gateway_plugins` still pushes only `Some` lookups; `proxy_http/` has no diff.
- No new `skills/04-review` page accepts fail-open for Gateway/EGC-global plugin references.
