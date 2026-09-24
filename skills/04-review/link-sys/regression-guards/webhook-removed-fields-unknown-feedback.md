---
name: webhook-removed-fields-unknown-feedback
description: Use when reviewing removed Webhook LinkSys fields and unknown-field diagnostics.
knowledge_type: regression-guards
---

# Removed Webhook fields produce unknown-field feedback

## Conclusion and scope

`WebhookServiceConfig` recognizes only the current `request.*` contract. Old
`auth`, `defaultHeaders`, and `requestMethod` values have no runtime effect.
Direct Kubernetes, filesystem, and etcd ingest can retain their unknown-field
paths for the advisory `UnknownConfigFields` condition and diagnostics. Admin
write returns response warnings; Admin validate returns the paths in the JSON
`unknownFields` field. Admin writes then persist a typed round-trip that drops those
keys, so a later status or diagnostics read does not retain that Admin warning.
Admission remains lenient.

## Rationale and evidence

The former typed `Option<Value>` placeholders hid these names from the schema
walk while the Gateway built requests from `conn`, `request`, and
`resolved_secrets`. The placeholders and warning detector were removed from
`resources/link_sys/webhook.rs` and the Controller LinkSys handler. The LinkSys
CRD preserves unknown `config` keys, so removing its obsolete `requestMethod`
property does not prune the key before the Controller sees it.

## Verification and re-evaluation

`schema_check/tests.rs` and `conf_center/common.rs` assert that all three old
field paths are reported. The Admin write boundary is documented in
`skills/01-architecture/01-controller/02-admin-api.md`. Re-evaluate if LinkSys
CRD unknown-field preservation, ingress schema walking, or Admin persistence
changes. The separate unused `WebhookAuth` type and shared CRD auth schema are tracked in
`tasks/todo/retire-unused-webhook-auth-type-and-schema.md`.

## Resolution references

- `review-results/controller-gateway-overengineering-20260924-031121/ISSUE-001`.
