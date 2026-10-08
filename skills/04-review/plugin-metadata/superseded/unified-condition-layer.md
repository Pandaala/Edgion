---
name: unified-condition-layer
description: Use when reviewing a second predicate enum, Include/Exclude, RestrictionSource, wildcard values, MtlsSan-as-accessor, keyNotExist/keyNotMatch, wrapping run/skip, or a default polarity/Skip; one Condition enum, grouping named by combination, KeyGet stays local.
status: accepted-tradeoff
finding_id: condition-layer-s15
closed: 2026-08-23
knowledge_type: superseded
---

# Unified Condition Layer — DESIGN-TRADEOFF

> **Domain clarification (2026-10-08):** the authoritative one-`Condition` rule
> applies to request predicates. Static WAF rule-metadata selection cannot consume
> `KeyGet`/JWT/secret/time variants or the Gateway request evaluator, and importing
> resources into the external-only WAF leaf would create a dependency cycle.
> Its bounded schema/decoder/validator/evaluator therefore has one canonical
> configuration-only owner in `edgion-waf`, reused by every configuration consumer.
> It has no request bindings or request-time evaluation. This structural
> re-evaluation leaves every request consumer's `Condition`/`ConditionSet` mandate
> intact; it permits no second request engine or importing-crate WAF selector.

> **Superseded grouping shape (2026-08-24):** the one-`Condition` rule remains
> authoritative, but the flat `skip`/`run`, RequestRestriction `anyOf`, and RAP
> `allOf` containers described below were replaced by the shared bounded
> `ConditionSet`. See
> `tasks/pending/condition-set-dnf-unification/01-design.md` and decision CSD-1.
> The text below is retained as the historical rationale for
> `condition-layer-s15`, not as current configuration guidance.

## Conclusion

There is one `Condition` enum. Every consumer accepts every variant. Grouping
is named by how elements combine: `PluginConditions.skip` is OR, `.run` is
AND; `RequestRestriction.anyOf` is OR of groups and each group's `allOf` is
AND; `RequestAccessPolicy` stays a conjunctive `allOf`. A container field
name must state that combination (`anyOf` / `allOf`, not `ruleGroups` /
`rules`). Match-only logging uses the compiled `descriptor`; evaluate stays
`Option<&mut PluginLog>`. `KeyGet` remains a local synchronous I/O-free read
(see [keyget-is-local-io-free.md](../mandatory-design/keyget-is-local-io-free.md)).

Fix suggestions that restore a deleted type, add a second predicate enum,
give this layer a default, or wrap `run` / `skip` in another object are
**not accepted**.

## Core Rationale

**1. One `Condition` enum; every consumer accepts every variant.**

`AccessCondition`, `IncludeCondition`, and `ExcludeCondition` were merged
into `Condition` (D-9). There is no per-plugin variant allow-list. Schema
and evaluator live in
`edgion-resources/src/resources/edgion_plugins/conditions.rs`
(`Condition`, `MatchCore`, `RuleGroup`) and
`edgion-gateway/src/plugins/runtime/conditions/`. A new predicate reuses
`Condition`; it does not add a plugin-local enum, `Include`/`Exclude`,
`RestrictionSource`, or wildcard `values`.

**2. A container field name must state how its elements combine.**

That is the naming criterion (D-11), not nesting depth. `run` already means
"run if all"; `skip` already means "skip if any". `RequestRestriction`
renamed `ruleGroups` → `anyOf` and `rules` → `allOf` because those names
said only "a list". Wrapping the already-named `run` / `skip` arrays in an
`allOf` / `anyOf` object would add a level for nothing (D-21).

**3. Match-only logging; evaluate stays optional.**

Each compiled condition carries a runtime-only `descriptor`
(`<ordinal>:<type>[:<accessor>]`). A match is pushed into `PluginLog`; a
non-match logs nothing. Body-phase and dye evaluation stay silent.
`evaluate` takes `Option<&mut PluginLog>` so the hot path can omit the log
without a second contract.

## Fix Suggestions Not Accepted

- Make `MtlsSan` a `KeyGet` accessor — rejected (D-8). SAN is multi-valued
  and needs `sanType`. `JwtClaim` and mTLS are `Condition` variants
  (`JwtClaimMatch`, `MtlsMatch`). `KeyGet` gained only `RequestPath`.
- Restore wildcards (`prefix*`, `wildcard_patterns`) — rejected (D-7).
  Regex covers them (`prefix*` → `^prefix.*`). Exact `values` still use a
  HashSet above the threshold on `KeyMatch`.
- Per-plugin variant allow-lists — rejected (D-9). A `Condition` means the
  same thing everywhere it appears.
- Push `considered > 0` into the shared layer — rejected. S13 deleted it;
  it was RequestRestriction-only Skip accounting, and Skip is gone.
- Add `keyNotExist` / `keyNotMatch` — rejected (D-15). Use `negate: true`.
- Wrap `run` / `skip` in an `allOf` / `anyOf` object — rejected (D-11 /
  D-21). `run` is already AND and `skip` is already OR; wrapping adds a
  level for nothing.
- Give anything in this layer a default polarity or a default Skip —
  rejected. `RequestRestriction.mode` is required with no default. Skip is
  not a `Condition` outcome.
- Flatten `SecretMatch` / `HmacAuth` — rejected (D-25). HMAC YAML keeps
  `HmacMatchConfig` fields as siblings of `type: hmacAuth`; `SecretMatch`
  stays a `Condition` variant. `deny_unknown_fields` on those structs is
  load-bearing.

## Re-evaluation Triggers

Re-open this decision only if:

- A new consumer cannot accept every `Condition` variant and has a
  documented structural reason (not taste) that a shared enum cannot express.
- `KeyGet` itself is redesigned so a variant is no longer a local single
  string read (see the KeyGet sibling).
- A real configuration is found that carries many wildcard entries and
  regex cost is measured as a hot-path problem (D-7 revisit).

## Reference Cases

- `condition-layer-s15` (2026-08-23) — this entry.
- `cl-10` (2026-08-24) — removed current-tense `RestrictionRule` / condition-layer
  `onMissing` references from the GeoRule rustdoc and grouped-list design guide; live upstream
  `onMissing` fields remain unrelated and unchanged.
- Decisions: `tasks/condition-layer/02-condition-unification/06-decisions.md`
  (D-7, D-8, D-9, D-11, D-15, D-21, D-25).
- Operator reference: `docs/en/user-guide/http-route/filters/edgion-plugins/conditions.md`.
- Unification: `skills/01-architecture/00-common/05-unification-points.md` §15.
- Sibling KeyGet locality: [keyget-is-local-io-free.md](../mandatory-design/keyget-is-local-io-free.md).

## Resolution references

- `condition-layer-s15` — 2026-08-23 (accepted tradeoff; grouping superseded 2026-08-24).
- `cl-10` — 2026-08-24.
