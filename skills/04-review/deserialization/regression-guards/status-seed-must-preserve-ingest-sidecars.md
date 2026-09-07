---
knowledge_type: regression-guards
---

# Status seeding must preserve ingest-only sidecars

## Conclusion

The FileSystem/etcd status seed performs a generic serde round trip to merge a
persisted `.status` document into a freshly parsed resource. Any ingest-only
state marked `#[serde(skip)]` must be captured before that round trip and
restored afterward.

The current required set is the `spec.unknown_fields` sidecar on
`EdgionPlugins`, `EdgionStreamPlugins`, `EdgionConfigData`, and `LinkSys`.

## Core rationale

FileSystem/etcd parsing populates unknown-field findings before the resource
processor seeds existing status. Serde intentionally omits those findings from
both serialization and deserialization, so an unguarded whole-resource round
trip resets them. The RawStore may still retain its pre-processor copy, which
creates contradictory diagnostics: fleet metrics report findings while the
handler warning and `UnknownConfigFields` status condition disappear.

Status seeding must remain before handler preparse/parse so it does not erase
the many runtime caches built there. At this boundary, the four unknown-field
vectors are the only populated `#[serde(skip)]` fields, so preserving them in
the shared seed helper is the minimal fix.

## Fix suggestions not accepted

- Re-run unknown-field detection after status seeding: the processor no longer
  has the original raw YAML, and reserialization has already dropped the typo.
- Move status seeding after preparse/parse: the same round trip would then erase
  compiled matchers and other runtime-only state.
- Persist `unknown_fields`: the sidecar is diagnostic process state and must
  remain absent from CRDs and conf_sync payloads.
- Ignore the status discrepancy because RawStore metrics remain correct:
  resource status and controller logs are part of the same operator contract.

## Re-evaluation triggers

When a new ingest-only `#[serde(skip)]` field is populated before processor
status seeding, add it to the shared preservation boundary and its regression.
If status is later merged through a typed status accessor without a
whole-resource round trip, this preservation shim can be removed.

## Reference case

- UFD-03 in [`review knowledge`](../../SKILL.md)
- Regression: `seed_existing_status_preserves_all_unknown_field_sidecars`

## Resolution references

- `UFD-03` — 2026-08-22.
