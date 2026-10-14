---
name: deny-unknown-fields-credential-structs-forward-compat-tradeoff
description: Use when reviewing findings that compare Edgion against Envoy/xDS proto3 "preserve-and-ignore unknown fields" and claim "serde deny_unknown_fields on LinkSys credential/auth sub-structs (SecretSlot / RedisAuth / EtcdAuth / ElasticsearchAuth / GlobalConnectionIpRestrictionEntry) breaks forward compatibility at the Controller→Gateway boundary / a future additive sub-struct field makes an old Gateway reject the whole LinkSys resource / decouple the inline-credential block from deny_unknown_fields via a custom validator or a separate wire struct / at minimum add a definition-site comment". Covers why deny_unknown_fields is intentional (it is the inline-credential-injection defense) and why the decoupling fixes are wide cross-layer features, not narrow review fixes.
status: accepted-tradeoff
finding_id: resource-05
closed: 2026-06-14
knowledge_type: intentional-design
---

# deny_unknown_fields on LinkSys credential structs vs forward compat — ACCEPTED-TRADEOFF (the attribute is the inline-credential defense)

## Conclusion

Several LinkSys credential/auth sub-structs carry `#[serde(... deny_unknown_fields)]`:
`SecretSlot` (`edgion-resources/src/resources/common/secret_slot.rs:18`), `RedisAuth`
(`link_sys/redis.rs:54`), `EtcdAuth` (`link_sys/etcd.rs:66`), `ElasticsearchAuth`
(`link_sys/elasticsearch.rs:58`), and `GlobalConnectionIpRestrictionEntry`
(`edgion_stream_plugins/stream_plugins.rs:25`). Because Controller and Gateway share these
types via `edgion-resources` and the Gateway re-parses the conf_sync JSON with the same
`serde_json::from_str` path (`event_dispatch.rs:95/323`), a Controller-newer-than-Gateway
rolling upgrade that adds a benign new field to one of these structs would make the old
Gateway reject the whole `LinkSys` resource (parse error on the unknown field).

This is the sub-struct-field twin of `resource-01` (the strict `EdgionPlugin` enum) and is the
**same accepted design tradeoff**, not a defect and not a narrow review fix. `deny_unknown_fields`
here is load-bearing: it is the defense that blocks inline credentials (`password:`, `username:`,
`clientKey:`) from being written into the CRD — proven by the tests
`redis_auth_rejects_inline_password` / `redis_auth_rejects_inline_username` (`redis.rs:257-269`),
`secret_slot::rejects_unknown_field` (`secret_slot.rs:46-50`), and the etcd equivalents.

Fix suggestions of the form "remove `deny_unknown_fields` and add a custom forbidden-field
validator", "introduce a separate `deny_unknown_fields`-free wire struct for the
Controller→Gateway payload", or "at minimum annotate every `deny_unknown_fields` site" are
**not accepted** as a narrow review fix.

ARCH-051 removed the orphan `WebhookAuth` type; it did not relax any live auth
strictness. `WebhookTarget` now lives in `resources/common/external_http.rs` and
does not carry `deny_unknown_fields`, so it is not an example of this boundary.

## Core Rationale

**1. The attribute is the inline-credential defense, not just a strictness knob**

The security intent is explicit in the struct doc comments and the tests: all credentials MUST
arrive via `secretRef`, and inline `password` / `username` / `clientKey` keys MUST be rejected.
`deny_unknown_fields` is precisely what makes `secretRef:\n  name: r\npassword: leaked` fail to
deserialize. Removing it (suggested direction 1) would make an inline credential **silently
ignored** — the operator believes they configured a password, the Gateway dropped it, and the
integration runs unauthenticated. That is a security regression in the common case to buy
resilience in a rare one — the same shape of argument that closed `resource-01` (there: masking
operator typos; here: masking inline-credential misuse), only stronger because security is
directly at stake.

**2. The decoupling fixes are wide cross-layer features**

- Suggested direction 1 (custom validator) changes the security mechanism of 6+ structs in the
  shared `edgion-resources` schema and adds a new explicit-forbidden-field check in the
  validation pipeline, discarding the compile-/test-time guarantees the current attribute gives
  for free.
- Suggested direction 2 (separate ingest vs wire struct) is a large feature: a parallel struct
  family plus a Controller-side ingest→wire conversion step before serialization.

Both are medium-to-large cross-module work touching a core shared schema type. If ever
prioritized they go through brainstorming as a `tasks/todo/` feature, mirroring the
`resource-01` disposition.

**3. The exposure is rare and bounded by deployment hygiene**

The breakage only triggers in the version-skew window of a Controller-before-Gateway rolling
upgrade *and* only when that upgrade introduces a brand-new sub-struct field referenced by a
live `LinkSys`. Same-version clusters never hit it (Controller ingest validation runs the same
schema). Standard rollout practice — upgrade the data plane first, or gate adoption of any new
credential-struct field until the fleet is upgraded — avoids it entirely. This matches the
Envoy/Istio posture catalogued in `gateway-no-local-config-snapshot.md` and `resource-01`: a
real-but-narrow rolling-upgrade gap, not a baseline defect.

**4. Definition-site comments are unnecessary churn, this entry is the canonical visibility**

The "at minimum add a comment to each `deny_unknown_fields`" suggestion would touch 6 files in
the shared schema for documentation value only. The canonical, discoverable home for the
constraint is this `04-review` entry (found via `grep -rl deny_unknown_fields skills/04-review/`),
which keeps the diff empty and parallel `/fix-issue` runs conflict-free. The forward-compat
constraint is the same one already documented for `resource-01`.

## Fix Suggestions Not Accepted

- "Remove `deny_unknown_fields` from the credential structs and add a custom validator that
  rejects forbidden inline field names" — wide change across 6+ shared-schema structs; until the
  validator runs it reopens the inline-credential injection hole (silent drop). Promote to
  `tasks/todo/` via brainstorming if ever prioritized.
- "Introduce a separate `deny_unknown_fields`-free wire struct for the Controller→Gateway JSON
  and convert ingest→wire on the Controller" — a large cross-layer feature, not a narrow fix.
- "At minimum add a definition-site comment on every `deny_unknown_fields`" — touches 6 shared
  files for doc value only; this entry is the canonical, conflict-free record instead.
- "Make the Gateway parse each LinkSys resource independently so one bad field only drops that
  resource" — same failure-semantics change discussed in `resource-01`; belongs with the same
  forward-compat feature, not a standalone fix.

## Re-evaluation Triggers

Re-open (as a `tasks/todo/` feature, not a review fix) only if:

- A production incident shows a Controller-before-Gateway rolling upgrade losing live `LinkSys`
  config because a benign additive credential-struct field tripped `deny_unknown_fields`, in a
  topology that readiness probes + staged rollout could not cover.
- The inline-credential block is moved to an explicit validation step for an independent reason,
  at which point `deny_unknown_fields` no longer carries security weight and can be dropped from
  the wire-facing structs cheaply.
- The credential structs begin shipping out-of-tree / on a different release cadence than the
  Gateway binary, so Controller and Gateway can no longer be assumed to share the schema crate.

## Reference Cases

- `tasks/review-envoy/resource-05-link-sys-deny-unknown-fields-breaks-forward-compat.md` (closed 2026-06-14, ⚠️ accepted-tradeoff)
- `skills/04-review/resources/intentional-design/unknown-plugin-variant-strict-enum.md` (`resource-01`, the sibling plugin-enum forward-compat tradeoff)
- `skills/04-review/resources/compatibility-constraints/serde-ignored-blind-spots-schema-walk-instead.md` — the
  complementary detector for the opposite case: an operator-facing, `x-kubernetes-preserve-unknown-fields`
  subtree (plugin `config`) where `deny_unknown_fields` cannot be used at all (it forbids
  `#[serde(flatten)]`) and a hard reject is not the wanted policy anyway; that detector
  warns instead of rejecting and does not touch the credential-injection boundary described here.
- `skills/04-review/conf-sync/intentional-design/gateway-no-local-config-snapshot.md` (the Envoy-comparison rolling-upgrade baseline)
- `skills/04-review/deserialization/SKILL.md` ("use `deny_unknown_fields` on internal structs that should be stable and closed; omit on user-facing extension points")
- `edgion-resources/src/resources/common/secret_slot.rs:18` + tests `:46-50`; `link_sys/redis.rs:54` + tests `:257-269`
- `edgion-gateway/src/conf_sync/cache_client/event_dispatch.rs:95` (list-path `Vec<T>` parse) and `:323` (per-event watch parse)
