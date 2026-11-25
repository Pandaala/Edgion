# Unknown Config Field Detection

`EdgionPlugins`, `EdgionStreamPlugins`, the typed variants of `EdgionConfigData`, and typed
`LinkSys` provider configurations carry configuration below CRD nodes that preserve unknown fields.
The consequence is that the Kubernetes API server
does not prune a mistyped key inside those `config` blocks, and the Controller's Rust
deserialization is the only remaining gate. Without a dedicated check, an unrecognized key is
silently dropped: `denyRefs:` mistyped as `denyRefss:` leaves an `IpRestriction` entry with an empty
deny list, `Accepted: True`, and nothing anywhere says the rule was never loaded. For a
security-relevant plugin that is a fail-open, not a harmless no-op.

For `EdgionConfigData`, the detector checks the typed `KeyList`, `IpList`,
`RequestAccessUrlAllowList` and `Selector` payloads. ClusterRegionSetting rejects unknown payload fields during deserialization. Credential and
authentication sub-structures within typed policy data remain strict security boundaries.
`data.type: Misc` is the documented free-form escape hatch, so
arbitrary keys below `Misc.config` are intentionally not reported. Kubernetes prunes unknown
siblings at the structural `spec` and `data` levels before the Controller sees them; raw-body
FileSystem, etcd, Admin write, and validate ingresses can additionally report those envelope keys.

For `LinkSys`, the detector checks the typed `redis`, `etcd`, `elasticsearch`, `kafka`, `webhook`,
and `httpdns` provider bodies. Declared maps and arbitrary-JSON values remain open below their
boundary; for example, the contents of a known JSON compatibility field are not interpreted as a
LinkSys schema. Unknown siblings beside an open field are still reported.

This document is the operator guide to the detector that catches it.

## The position, stated plainly

Unknown non-credential fields in ordinary supported configurations are advisory:
`UnknownConfigFields` reports them without changing `Accepted` or blocking sync.
Credential substructures remain strict. Security-sensitive ConfigData variants
(`ProxyProtocolTrust`, `WafRuleBundle`, and `WafPolicy`) reject unknown config fields.
For an admission gate, use `POST /api/v1/validate/{kind}` before apply.

The Admin API rejects a POST or PUT with HTTP 400 if a known HTTP plugin's
`config` cannot be decoded, for example when `LdapAuth.ldapPort` contains text.
The stored resource and its serving configuration remain intact. The read-only
validate endpoint reports this error in `validationErrors` even when the
Controller runs in Kubernetes mode without a local CRD validator. Kubernetes
applies its installed CRD when a resource is submitted to its API. If an invalid
config is already present in a filesystem, etcd or Kubernetes source, the
Controller can still read that `EdgionPlugins` and the other objects in its list.
It reports `Accepted=False` with the stage and zero-based entry index. The
Gateway rejects traffic using the invalid plugin chain even if that entry is
disabled. Correct the indicated entry in the submitted manifest and reapply it.

Unknown extension **types** have different semantics. LinkSys `spec.type` and
EdgionConfigData `spec.data.type` are open CRD strings, but a type unsupported by
the running binary reports `Accepted=False/Invalid`. The typed decoder discards
its name and config, and syncs an invalid marker so previous clients or data indexes
are withdrawn. ConfigData overlay consumers fall back to their base; strict trust
and required WAF references remain invalid. The marker retains an empty `config: {}`
envelope. Admin writes persist that marker, so correct the type or upgrade Edgion,
then reapply the original configuration to recover. Nested enums and known
configuration validation remain unchanged.

## Where a finding shows up

An operator can see a finding in four places:

1. **A `warn!` log line** on the Controller, at the moment the resource is applied, carrying
   the resource's namespace/name and a bounded summary of unrecognized key paths: at most 20
   concrete paths plus an omission marker, with each path capped at 512 bytes.
2. **The `UnknownConfigFields` status condition** on the resource itself:

   ```yaml
   status:
     conditions:
       - type: UnknownConfigFields
         status: "True"
         reason: UnknownFieldsDetected
         message: "requestPlugins[0].config.denyRefss"
   ```

   `kubectl describe` shows this like any other condition. It does **not** affect `Accepted`
   (which keeps meaning "resource valid and synced") and it does **not** feed the
   `edgion_controller_resource_abnormal` metric — an operator typo is not resource
   ill-health. The condition is removed automatically on the next apply once the key is
   fixed; it is never left wedged at a stale `True`.

3. **`GET /api/v1/diagnostics/unknown-fields`** — the fleet-wide answer, across every
   `EdgionPlugins`, `EdgionStreamPlugins`, `EdgionConfigData`, and `LinkSys` resource the Controller currently holds:

   ```json
   {
     "success": true,
     "data": {
       "total": 2,
       "truncated": false,
       "items": [
         {
           "kind": "EdgionPlugins",
           "namespace": "default",
           "name": "auth-chain",
           "paths": ["requestPlugins[0].config.denyRefss"]
         }
       ]
     }
   }
   ```

   Requires the standard Admin API bearer token. `total` is the true count before the
   100-item response cap; `truncated: true` when the cap bites.

4. **`warnings` in the Admin API write response** — when you `POST`/`PUT` an `EdgionPlugins`,
   `EdgionStreamPlugins`, `EdgionConfigData`, or `LinkSys` document through the Admin API and it contains an unrecognized
   key, the response carries it immediately:

   ```json
   {
     "success": true,
     "data": "EdgionPlugins created",
     "warnings": ["requestPlugins[0].config.denyRefss"]
   }
   ```

   This is the one ingress with no second chance to report the finding elsewhere: the Admin
   API write path persists a typed round-trip, which strips unrecognized keys from the
   document **before** it is written to storage. The FS loader, the etcd loader, and the
   Kubernetes watcher all catch the same class of finding on their own ingress, but if you
   wrote through the Admin API, this response is the only place the finding appears. The
   write still succeeds (`success: true`) — this is advisory, matching the warn-not-block
   position above.

## Building your own blocking gate

`POST /api/v1/validate/{kind}` gives you a synchronous, side-effect-free verdict on a
document exactly as you would apply it — nothing is persisted, no resource version is
bumped:

```
POST /api/v1/validate/EdgionPlugins
<the document, YAML or JSON, exactly as it would be applied>

{
  "success": true,
  "data": {
    "ok": false,
    "unknownFieldCheck": "performed",
    "unknownFields": ["requestPlugins[0].config.denyRefss"],
    "schemaErrors": [],
    "validationErrors": []
  }
}
```

`unknownFieldCheck` is `"performed"` for `EdgionPlugins`, `EdgionStreamPlugins`,
`EdgionConfigData`, and `LinkSys`; it is `"unsupported"` for every other kind. Always
check this field rather than assuming an empty `unknownFields` means "checked, nothing
found": for an unsupported kind it means "not checked at all". Wire this endpoint into
whatever gate you want to enforce blocking with — a `ValidatingAdmissionWebhook`, a CI step
in your GitOps pipeline, or a pre-`kubectl apply` check in your own tooling.

## Related documentation

- [EdgionPlugins — HTTP Plugins](https://github.com/Pandaala/Edgion/blob/main/skills/02-features/03-resources/13-edgion-plugins.md)
  and [EdgionStreamPlugins — Stream Plugins](https://github.com/Pandaala/Edgion/blob/main/skills/02-features/03-resources/14-edgion-stream-plugins.md)
  — the `UnknownConfigFields` condition in context.
- [EdgionConfigData](https://github.com/Pandaala/Edgion/blob/main/skills/01-architecture/05-resources/15-edgion-config-data.md)
  — typed variants, the open `Misc` boundary, and Controller processing.
- [LinkSys](https://github.com/Pandaala/Edgion/blob/main/skills/02-features/03-resources/18-link-sys.md)
  — provider schemas, open boundaries, and status behavior.
- [Admin API — diagnostics and validate endpoints](https://github.com/Pandaala/Edgion/blob/main/skills/01-architecture/01-controller/02-admin-api.md)
  — the full endpoint reference.
- [Plugin Policy (Allowlist / Denylist)](./plugin-policy.md) — GatewayClass-scoped execution control in EdgionGatewayConfig;
  this is enforced policy, distinct from unknown-field advisory status.
