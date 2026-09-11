# GatewayClass plugin execution policy

Configure plugin execution in `EdgionGatewayConfig.spec.pluginPolicy`. The
`GatewayClass.spec.parametersRef` binding selects the policy for its Gateways.
The same EdgionPlugins or EdgionStreamPlugins resource can run under different
policies in different classes. Edits take effect through normal configuration
sync without restarting Controller, Gateway, or existing listeners.

## Configure and bind

```yaml
apiVersion: edgion.io/v1alpha1
kind: EdgionGatewayConfig
metadata:
  name: public-policy
spec:
  pluginPolicy:
    deny:
      - name: http/Wasm
      - name: http/Dsl
        action: block
        blockStatus: 451
      - name: stream/ConnectionRateLimit
        action: bypass
    defaultAction: allow
    deniedAction: block
    blockStatus: 503
---
apiVersion: gateway.networking.k8s.io/v1
kind: GatewayClass
metadata:
  name: public
spec:
  controllerName: edgion.io/gateway-controller
  parametersRef:
    group: edgion.io
    kind: EdgionGatewayConfig
    name: public-policy
```

A Gateway using `gatewayClassName: public` receives this policy. Change the EGC
resource to update all Gateways in that class. To cover multiple classes, bind
them to the same EGC. An unreferenced EGC has no effect on plugin execution.
Install the updated structural CRD before setting `pluginPolicy` in Kubernetes;
otherwise an API server using the old schema can prune the field.

The current Kubernetes Controller startup setting `conf_center.gateway_class`
still selects one GatewayClass per Controller. Policy binding does not widen that
resource ownership. Multiple-class publication is available to configuration
centers that supply those Gateways, including filesystem mode.

Names are exact, case-sensitive `<family>/<Type>` values: `http` covers all four
HTTP stages, `stream` covers the connection stage, and `tls-route` covers the
TLSRoute stage. For example, `http/IpRestriction` and
`tls-route/IpRestriction` are distinct names. Unknown names and duplicates are
invalid; there are no wildcard names.

`deny` wins over `allow`. Supplying `allow` enables an allow list across all three
families; `allow: []` restricts every name. Without an allow list, unmatched names
follow `defaultAction` (`allow` by default). Restricted entries follow
`deniedAction` (`block` by default), with optional per-deny-entry overrides.
`blockStatus` must be between 400 and 599. A deny entry using or inheriting
`bypass` cannot also specify `blockStatus`.

The [field reference](../../../skills/02-features/02-config/02-edgion-gateway-config.md#specpluginpolicy)
contains the complete schema. There is no Controller startup policy setting;
`validation.plugin_policy` is rejected, with no compatibility alias.

## Runtime behavior

**Block** rejects the plugin chain unconditionally, ahead of per-entry conditions
and invalid-configuration gates. HTTP uses the configured status and the generic
body `{"message":"plugin disabled by policy"}`. Even a response-stage Block is
known at request admission. Within one resource, the first blocked entry in
request, response-header, response-body, then async-response order determines
the status. Stream and TLSRoute Block deny admission; an HTTP status has no
meaning at those stages.

**Bypass** skips the restricted implementation and continues the chain. Bypassing
an authentication plugin also removes its authentication check. Policy does not
remove code from the binary, govern native Gateway API filters, or govern the
separate resource-level native WAF configuration.

The Gateway constructs entries only when a serving scope permits and references
them. Allowed scopes share stateful implementations of the same source generation.
A policy edit releases implementations no longer needed by current scopes;
already admitted requests can retain their old instances until completion.

Invalid DSL/WAF updates retain a compatible previous valid source when permitted
by existing reload rules. Current restrictions apply to both the incoming source
and the retained candidate. A new Block cannot disappear behind last-known-good
retention. Relaxing policy while the latest program is still invalid cannot
resurrect an excluded old entry; that case can return the ordinary configuration
error 500 until the source is repaired. Cross-namespace mirror and other
non-retainable validation errors keep their fail-closed replacement rules.

## Update boundaries

- No parametersRef, or a present EGC with no `pluginPolicy`, means allow all.
- A referenced EGC that is missing, deleted, or semantically invalid makes the
  scope unavailable: HTTP returns 503 and new stream admissions are denied.
  A resource rejected before typed publication leaves the prior typed input in
  place; it is not a successful policy update.
- Each HTTP request captures the actual logical Gateway scope. Later response,
  body and backend plugin lookups use that scope. New keep-alive/H2 requests
  observe new publications. Existing requests and stream/UDP sessions are not
  forcibly revoked. Backend ExtensionRefs run only when the selected backend is
  first used, under that captured scope. A restriction on an unselected backend
  candidate does not reject the request before selection.
- A shared physical listener runs a connection-stage chain only when every
  serving Gateway has an equivalent effective chain. Conflicts deny new
  connections. Configured early RequestId entries wait for route ownership when
  multiple logical Gateways share the listener.
- Configuration delivery is asynchronous across processes and resource kinds.
  This does not provide a fleet-wide atomic switch or a multi-resource transaction.

## Observe and verify

The Controller validates `pluginPolicy` on the EGC and reports configuration
errors there. Shared plugin resources retain source-validation diagnostics;
they do not carry a global `PluginPolicyRestricted` condition. Controller status
is not an acknowledgement that every Gateway has applied a policy.

HTTP rejection and bypass use the normal `PluginPolicyBlock` / `PluginPolicyBypass`
access-log entries. Gateway counters
`edgion_gateway_plugin_policy_blocked_total` and
`edgion_gateway_plugin_policy_bypassed_total` use bounded qualified-name labels.
Stream results remain in the existing stream log path. Verify the desired
behavior with fresh requests or connections to each affected Gateway class.
The admin store-statistics endpoint includes publication ownership counts and
inventories for cleanup diagnostics.
