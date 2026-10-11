---
id: ARCH-056
status: confirmed
kind: architecture-debt
priority: P2
partition: PART-215
analyst: /root
baseline: BASELINE-AUDIT-20261010-C.json
---

# Retire the L4 plugin's obsolete HTTP-schema migration detector

`ConnectionIpRestrictionRules` owns current L4 allow/deny groups, references and
default action, but also retains every unrecognized config value in a private
flattened `BTreeMap<String, Value>` solely to detect three former HTTP fields.
The map is hidden from schemars, but is serialized when nonempty; cloning a
configuration also clones this retained payload. `LEGACY_HTTP_FIELDS` and
`legacy_http_fields` are at lines 23 and 66 of
`edgion-resources/src/resources/edgion_stream_plugins/connection_ip_restriction_rules.rs`.

This obsolete schema remains an executable cross-component contract. The
Controller stream handler defines a marker, a special condition reason and a
migration document path, detects the fields before current structural validation,
and strips/parses the marker again while setting status (lines 27–38, 153–164 and
279–291). Gateway `StreamPluginRuntime::build_entry` at line 152 independently
rejects their presence. Resources, Controller and Gateway tests preserve the old
field behavior. The referenced `docs/migrations/ec4e000-ip-restriction-stream-schema.md`
does not exist in the current tree. A supported allow-list plus `message` is an
actual distinguishing example: it currently receives `LegacySchemaDeprecated`
and a runtime deny gate even though `message` has no L4 meaning.

This logic once supplied an understandable migration diagnostic instead of a
watcher deserialization failure. However, Edgion is pre-release and the root
project rules explicitly reject unrequested legacy compatibility scaffolding.
The original review decision
`skills/04-review/ip-restriction/regression-guards/stream-preparse-uses-plugin-validation.md`
expressly identifies this detector as pre-release removal debt and asks that
its branches and tests be removed together when implementation is in scope.
Its accepted conclusion protects current-schema stateless validation, not the
migration detector.

Remove the private payload catch-all, field-name detector, Controller migration
marker/reason/document logic and Gateway legacy-field predicate as one bounded
retirement. Use the existing canonical schema-walk diagnostic at raw operator
ingress for removed keys. `EdgionStreamPluginsSpec::unknown_fields` carries only
paths and is neither serialized nor exposed in the schema; Kubernetes, FS/etcd
and Admin ingress already feed this mechanism. The schemars skip on the current
catch-all means the existing schema walker can already report these keys; this
proposal does not claim its current implementation is blind to them.

The intentional behavior change must be explicit: a valid current L4 rule with
an old HTTP field becomes a valid rule with advisory unknown-field feedback,
rather than a special migration rejection. An old field alone still cannot
replace required allow/deny rules or references. Keep current structural errors
invalid and fail closed, unsupported types invalid even when disabled or bypassed,
known disabled entries inert, partial-good CIDRs and deny-first/whitelist semantics,
typed reference authorization, physical peer ownership and stage separation.
Preserve the Gateway front gate before any later stateful limiter for every
remaining invalid configuration. Do not change HTTP `IpRestrictionConfig` fields,
matcher construction, canonical diagnostics or unrelated status contracts.

The alternative is to keep a special forbidden-field policy, but that retains
three-way historical coupling without a current product requirement. Retirement
reduces serialized/cloned cold configuration payload and removes dedicated
cross-component policy; no request-path benchmark or production incident is
claimed. Future verification should replace legacy-only tests with exact advisory
paths and omission of unknown values from typed serialization; cover valid rules
with removed keys, missing current rules, disabled entries, unsupported types,
and remaining invalid-front-gate ordering. Current tests were read, not executed.

Knowledge searched: the original stateless-validation decision above, the schema
walk/serde flatten compatibility decision, current stream architecture and feature
schema, current Controller/Gateway branches and their tests, and repository task
and migration paths. No surviving migration document or matching live task was
found. This read-only report grants no implementation authority.
