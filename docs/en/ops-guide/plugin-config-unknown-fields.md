# Unknown Config Field Detection

`EdgionPlugins` and `EdgionStreamPlugins` model each plugin's `config` block as
`x-kubernetes-preserve-unknown-fields` in the CRD — deliberately, so the schema does not need
to know about every plugin's config shape. The consequence is that the Kubernetes API server
never prunes a mistyped key inside a plugin `config` block, and the Controller's Rust
deserialization is the only remaining gate. Without a dedicated check, an unrecognized key is
silently dropped: `deny:` mistyped as `denys:` leaves an `IpRestriction` entry with an empty
deny list, `Accepted: True`, and nothing anywhere says the rule was never loaded. For a
security-relevant plugin that is a fail-open, not a harmless no-op.

This document is the operator guide to the detector that catches it.

## The position, stated plainly

**We only warn, we do not block. Blocking a malformed config belongs before apply.**

The Controller never rejects a resource because of an unrecognized config key. `Accepted`
stays untouched, the config still syncs to the Gateway exactly as it would have without this
feature, and the mistyped key is still ignored — the only change is that somebody is now
told. There is no strict/reject mode, and none is planned: rejecting raises questions this
feature deliberately does not answer (what happens to a resource that was already applied and
syncing before rejection was turned on, what happens on Controller restart when the whole
store is re-ingested, what each conf_center mode should do when the rejection has no operator
to report to). If you want blocking, build it yourself on `POST /api/v1/validate/{kind}` (see
below) — a `ValidatingAdmissionWebhook`, a CI check, or a GitOps pre-merge step, all of which
run before the Controller ever sees the object.

## Where a finding shows up

An operator can see a finding in four places:

1. **A `warn!` log line** on the Controller, at the moment the resource is applied, carrying
   the resource's namespace/name and the full list of unrecognized key paths.
2. **The `UnknownConfigFields` status condition** on the resource itself:

   ```yaml
   status:
     conditions:
       - type: UnknownConfigFields
         status: "True"
         reason: UnknownFieldsDetected
         message: "requestPlugins[0].config.denys"
   ```

   `kubectl describe` shows this like any other condition. It does **not** affect `Accepted`
   (which keeps meaning "resource valid and synced") and it does **not** feed the
   `edgion_controller_resource_abnormal` metric — an operator typo is not resource
   ill-health. The condition is removed automatically on the next apply once the key is
   fixed; it is never left wedged at a stale `True`.

3. **`GET /api/v1/diagnostics/unknown-fields`** — the fleet-wide answer, across every
   `EdgionPlugins` / `EdgionStreamPlugins` resource the Controller currently holds:

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
           "paths": ["requestPlugins[0].config.denys"]
         }
       ]
     }
   }
   ```

   Requires the standard Admin API bearer token. `total` is the true count before the
   100-item response cap; `truncated: true` when the cap bites.

4. **`warnings` in the Admin API write response** — when you `POST`/`PUT` an `EdgionPlugins`
   or `EdgionStreamPlugins` document through the Admin API and it contains an unrecognized
   key, the response carries it immediately:

   ```json
   {
     "success": true,
     "data": "EdgionPlugins created",
     "warnings": ["requestPlugins[0].config.denys"]
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
    "unknownFields": ["requestPlugins[0].config.denys"],
    "schemaErrors": [],
    "validationErrors": []
  }
}
```

`unknownFieldCheck` is `"performed"` only for `EdgionPlugins` and `EdgionStreamPlugins` — the
only two kinds with a wired schema walk — and `"unsupported"` for every other kind. Always
check this field rather than assuming an empty `unknownFields` means "checked, nothing
found": for an unsupported kind it means "not checked at all". Wire this endpoint into
whatever gate you want to enforce blocking with — a `ValidatingAdmissionWebhook`, a CI step
in your GitOps pipeline, or a pre-`kubectl apply` check in your own tooling.

## Related documentation

- [EdgionPlugins — HTTP Plugins](https://github.com/Pandaala/Edgion/blob/main/skills/02-features/03-resources/13-edgion-plugins.md)
  and [EdgionStreamPlugins — Stream Plugins](https://github.com/Pandaala/Edgion/blob/main/skills/02-features/03-resources/14-edgion-stream-plugins.md)
  — the `UnknownConfigFields` condition in context.
- [Admin API — diagnostics and validate endpoints](https://github.com/Pandaala/Edgion/blob/main/skills/01-architecture/01-controller/02-admin-api.md)
  — the full endpoint reference.
- [Plugin Policy (Allowlist / Denylist)](./plugin-policy.md) — the neighboring
  operator-visible plugin-resource advisory feature this detector's condition/metric
  discipline mirrors.
