---
knowledge_type: regression-guards
---

# Identity-model quota uses the effective logical name

AiProxy admits token quota before selecting an AI backend, but after explicit
`modelRoutes` rewrite. The quota dimension is the effective `logical_model` at
admission: normally the client's name/alias, or the authorized rewrite target.
A quota `Downgrade` replaces it and repeats admission. Backend failover maps
that effective name to an attempt-local provider model without changing the
quota identity. There is no candidate-independent provider canonical name.

Different logical aliases therefore normally have separate `identityModel`
buckets even when they resolve to the same provider model. The schema's 32
aliases are patterns, including `prefix*`; they do not bound client-requested
names or quota buckets to 33. An explicit rewrite can deliberately collapse
names onto one effective model, subject to its header/match rules. Use
`identity` within the same AiProxy entry for an aggregate cap across names.
`Downgrade` permits only `identityModel`, so account for alias/wildcard expansion.

Entry scope is EdgionPlugins Namespace/name plus plugin-entry alias or index.
Referencing one entry across routes shares its buckets; duplicating the config
in another entry does not. An empty identity key is global within that entry,
not across every entry. No separate shared-budget ID is implemented.

Do not normalize through the first backend candidate. That makes identity
depend on route order and breaks failover mappings.

## Source evidence and verification limits

- `edgion-gateway/src/ai/http/proxy.rs`: model-route decision and
  `replace_logical_model` precede quota admission.
- `edgion-gateway/src/ai/quota/runtime.rs`: entry scope and effective model
  feed target construction.
- `edgion-resources/src/resources/edgion_backend.rs`: bounded alias patterns
  include suffix wildcards, not merely a finite set of exact names.
- `gw-04` originally documented requested-name identity on 2026-08-09.
  Task 04 statically revalidated current source on 2026-10-06 and corrected
  the pre-rewrite wording and obsolete 33-bucket bound. No new runtime tests
  are implied by this documentation correction.

Re-evaluate on model-route ordering, alias syntax, entry identity or quota-key
changes. This page does not assert invoice-grade or zero-overshoot enforcement.
