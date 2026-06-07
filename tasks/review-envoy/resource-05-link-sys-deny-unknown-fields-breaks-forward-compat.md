---
module: resource-config-model
title: deny_unknown_fields on LinkSys auth structs breaks forward compatibility at the Controller→Gateway boundary
severity: medium
envoy_reference: xDS / proto3 "be liberal in what you accept" — unknown fields in resource payloads are preserved, not rejected; a newer control plane can send enriched payloads to an older data plane without breaking it
---

# deny_unknown_fields on LinkSys auth structs breaks forward compatibility at the Controller→Gateway boundary

## Envoy's design (the proven baseline)

xDS uses proto3 whose unknown-field rule is "preserve and ignore." When a newer management plane
sends a resource that contains a field the current data plane binary does not know, the data plane
silently ignores the unknown field and accepts the resource. This is fundamental to the rolling
upgrade model: the management plane can be upgraded first; older data planes continue to parse and
apply the resources without error, simply ignoring the new fields.

When Edgion uses JSON (serde_json) as the wire format over gRPC, the serde `deny_unknown_fields`
attribute is the direct opposite of proto3's model: it makes the deserializer reject any JSON object
that contains a key not present in the Rust struct, regardless of whether the unknown field is load-
bearing.

## Edgion's current design

Several LinkSys credential/auth types carry `#[serde(... deny_unknown_fields)]`:

- `src/types/resources/link_sys/redis.rs:54` — `RedisAuth` (not `RedisTlsCerts`; `#[serde(deny_unknown_fields)]` on the auth struct)
- `src/types/resources/link_sys/etcd.rs:66` — `EtcdTlsCerts`
- `src/types/resources/link_sys/elasticsearch.rs:58` — `ElasticsearchAuth` (tag + deny_unknown_fields on enum)
- `src/types/resources/link_sys/webhook.rs:20` — `WebhookAuth`; `webhook.rs:63` — `WebhookTarget` (not a second `WebhookAuth` variant)
- `src/types/resources/common/secret_slot.rs:18` — `SecretSlot` (used across multiple resource types)
- `src/types/resources/edgion_stream_plugins/stream_plugins.rs:21` — stream plugin config struct

`SecretSlot` in particular is used in multiple resources beyond LinkSys. `deny_unknown_fields` on
`SecretSlot` means that if a future version of the struct adds any new optional field (e.g., an
`expires_at` hint or a `rotation_policy`), any Gateway binary that does not have that field will
reject the entire parent resource.

The `deny_unknown_fields` annotations were added intentionally to enforce that inline secret values
(e.g., `clientKey`, `password`) are never accepted — the comments in the test files confirm this
security intent. The security goal is valid, but `deny_unknown_fields` is too broad a mechanism: it
prevents both inline secret injection AND forward-compat field addition.

## The problem / gap

The security rationale (block inline credentials) and the forward-compat concern are conflated into
a single serde attribute. When a new field is added to `SecretSlot`, `RedisTlsCerts`, or
`WebhookAuth` in a future Controller version:

1. The Controller serializes the resource to JSON including the new field.
2. The Gateway, running the old binary, deserializes the `LinkSys` resource via the gRPC list/watch
   path (`event_dispatch.rs:95/323`).
3. serde rejects the JSON object because of `deny_unknown_fields` on the auth/TLS sub-struct.
4. The entire `LinkSys` resource is dropped with a parse error, breaking the corresponding plugin
   integrations (Elasticsearch access logs, Redis rate limiting, etcd, webhooks) on the old Gateway.

Unlike finding 01 (plugin type enum mismatch), this is about sub-struct fields — even a completely
benign additive change to a credential struct can cause this breakage.

## Impact

- Any additive change to `SecretSlot`, `RedisTlsCerts`, `EtcdTlsCerts`, `ElasticsearchAuth`, or
  `WebhookAuth` field sets creates a hard Gateway version constraint.
- The developers of these structs must remember to keep both binaries in lockstep — there is no
  compiler or test that enforces this.
- If the rolling upgrade of the Gateway is slow (large cluster), the window where the Controller is
  new and Gateways are old could exceed minutes, during which all access-log and rate-limit
  integrations are broken.

## Verification note

Code confirmed. All cited `deny_unknown_fields` annotations exist. Struct name corrected above (see bullet list). `LinkSys` is confirmed to be synced to the Gateway via `ClientCache<LinkSys>` in `config_client.rs:27`; the Gateway deserializes the JSON using the same `serde_json::from_str` path, so `deny_unknown_fields` on `SecretSlot`, `RedisAuth`, `EtcdTlsCerts`, `ElasticsearchAuth`, and `WebhookAuth`/`WebhookTarget` will reject the parent `LinkSys` resource if the Controller sends any unknown field. Security intent of blocking inline credentials is confirmed by test comments in `redis.rs:257-264` and `etcd.rs:274,496,503`.

## Suggested direction

Decouple the inline-credential-block policy from `deny_unknown_fields`. Two approaches:

1. **Custom validator instead of deny_unknown_fields**: Remove `deny_unknown_fields` from security-
   sensitive structs and instead add a `validate()` step (already present in the pipeline) that
   explicitly checks for the presence of forbidden field names in the parsed JSON before the struct
   is stored. This preserves the security intent while restoring forward compatibility.

2. **Separate the schema boundary**: If `deny_unknown_fields` must stay for the K8s-API-ingest
   boundary (user→Controller), introduce a separate "wire" struct (without `deny_unknown_fields`)
   for the Controller→Gateway JSON payload. The Controller converts the validated ingest struct to
   the wire struct before serialization; the Gateway deserializes the wire struct and the inline-
   credential check is not needed because the Controller already stripped any inline values.

At minimum, add a comment on each `deny_unknown_fields` annotation explicitly documenting that any
future field addition requires a coordinated Gateway upgrade, so the constraint is visible at the
definition site.
