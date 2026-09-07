---
name: partially-invalid-not-route-exclusive
description: Use when reviewing a non-route resource that emits the PartiallyInvalid status condition; covers why a partially-resolvable collection is a sanctioned use of the condition rather than a vocabulary violation.
status: fixed
finding_id: docs-04
closed: 2026-08-04
knowledge_type: intentional-design
---

# `PartiallyInvalid` is not route-exclusive — DOCS-DRIFT (fixed)

## Conclusion

A non-route resource emitting `PartiallyInvalid` is **not** a status-vocabulary violation.
The condition covers any resource whose spec holds a collection that can be *partially*
resolved. Four emitter families exist today: `HTTPRoute` / `GRPCRoute` (per-rule backend
refs), `EdgionPlugins` / `EdgionStreamPlugins` (inline IpRestriction and
GlobalConnectionIpRestriction profile CIDR entries), `EdgionConfigData` (IpList entries), and
`EdgionBackend` (provider credential pool).

Findings of the form "`PartiallyInvalid` is documented as HTTPRoute/GRPCRoute only, so the
EdgionBackend handler violates the fixed condition vocabulary" are **not accepted** — the
documentation was stale, and it has been corrected rather than the handler.

## Core Rationale

**1. The route scoping was stale documentation, never a contract**

`skills/01-architecture/01-controller/10-status-management.md` §3 and the §Y line in
`skills/00-design-rules.md` both said "routes only". That was already false before the AI
work: `edgion_plugins.rs`, `edgion_stream_plugins.rs`, and `edgion_config_data.rs` reach the
condition through the shared `status_utils::partially_invalid_condition` helper rather than a
direct `condition_types::PARTIALLY_INVALID` reference, which is why a grep for the constant
misses them — and each is documented as an emitter in its own resource skill file. Both
canonical files now state the general rule.

**2. The semantics generalize cleanly; only the carrier is route-specific**

The condition means "part of a declared collection is unusable, the rest is live". For routes
the failing subset is carried in `spec.invalid_rule_indices` so the Gateway can skip those
rules. That field is a route transport detail, not part of the condition's contract — the
non-route emitters set the condition with no index vector and name the bad entries in the
condition message instead (the AI handler additionally sanitizes each credential alias; the
three IP emitters join their entry errors verbatim).

**3. Reusing the condition is what the vocabulary rule actually asks for**

The 🟡 rule in `00-design-rules.md` exists to stop authors inventing a new condition type per
resource. A partially-resolvable collection is precisely the case the existing type covers, so
reusing it is compliance, not deviation. The user-facing docs
(`docs/{en,zh-CN}/ai-gateway/edgion-backend.md`, `.../observability.md`) already documented
`PartiallyInvalid=True/InvalidCredentials` for EdgionBackend — the drift was confined to the
`skills/` layer.

**4. What is genuinely new about EdgionBackend (and was worth documenting)**

Two things, neither of them the condition type itself:

- It is the first emitter with a **resource-specific reason** (`InvalidCredentials`) instead
  of the shared `RuleInvalid`.
- It emits from **`parse()`, not `update_status()`**, because credential resolution is
  `async`. Every other emitter writes the condition in `update_status()`, which is what the
  surrounding "ref checks belong in `update_status()` only" rule assumes. This is a
  handler-local exception, not a new pattern to copy.

## Fix Suggestions Not Accepted

- Remove the `PARTIALLY_INVALID` emit from `edgion_backend.rs` to satisfy the documented
  "routes only" wording — inverts the fix direction; it would delete the operator's only
  signal that a usable credential subset is running degraded.
- Invent a resource-specific condition type (e.g. `CredentialsPartiallyResolved`) — exactly
  the per-resource condition proliferation the vocabulary rule forbids.
- Add `invalid_rule_indices` (or an analogue) to `EdgionBackend` for symmetry with routes —
  the field exists so the Gateway can skip rules by index; the AI credential pool has no such
  consumer, and the bounded message already names the omitted aliases.
- Move the AI emit into `update_status()` "for consistency" — that hook is sync; the async
  Secret resolution the condition reports cannot run there.

## Re-evaluation Triggers

Re-open this decision only if:

- The status vocabulary itself is renegotiated (e.g. Gateway API introduces a standard
  condition for partially-resolved references that Edgion should adopt instead).
- A resource wants `PartiallyInvalid` for something that is *not* a partially-resolvable
  collection — that is a genuine misuse and should be flagged.

Note: the `ResolvedRefs=True` value in the "some resolved, some failed" row documents current
behavior only. A separate open finding argues it should be `False` while `PartiallyInvalid`
stays `True`; if that lands, refresh the matrix in §4.2 — it does not affect this rule.

## Reference Cases

- `docs-04` (AI Gateway review batch, closed 2026-08-04) — docs-only fix; no code change.
- `skills/01-architecture/01-controller/10-status-management.md` §3 table and
  §4.2 "PartiallyInvalid on non-route resources"
- `skills/00-design-rules.md` §Y (CRD status condition vocabulary)
- `edgion-controller/src/conf_mgr/sync_runtime/resource_processor/handlers/edgion_backend.rs`
  (`parse`), `.../status_utils.rs` (`INVALID_CREDENTIALS`, `partially_invalid_condition`)
- Sibling emitters: `.../handlers/edgion_plugins.rs`, `.../edgion_stream_plugins.rs`,
  `.../edgion_config_data.rs` (all in `update_status`)

## Resolution references

- `docs-04` — 2026-08-04 (docs-only).
