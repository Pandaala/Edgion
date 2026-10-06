---
name: ai-guard-span-bounds-and-skip-semantics
description: Use when reviewing AiGuard span selection bounds, message/part skip-versus-unguardable semantics, or the paired ai-guard user documentation.
knowledge_type: regression-guards
---

# AiGuard span bounds and message-skip semantics

## Conclusion and scope

`edgion_gateway::ai::guards::request` distinguishes two outcomes while
selecting guarded spans, and the paired `ai-guard.md` user guides must state both
in each language:

- **Skipped** (the message or part contributes no span, the request proceeds):
  `messages[i].content` missing or of an unsupported JSON type; a non-object
  element inside a `content` array; a part whose `type` is missing, non-string,
  or not `text`; a `text` field that is missing or non-string.
- **Unguardable** (`SpanError::Unparseable`, handled by the `failOpen` policy):
  `role` missing, non-string, or duplicated; `content` duplicated; `type`
  duplicated; `text` duplicated when the unique `type` is `text`.

`maxSpanBytes` and `maxTotalSpanBytes` are `Option<usize>` with **no small
default**. When omitted each inherits the resolved request-body cap, whose
default is 32 MiB, and therefore may exceed the 8 MiB ceiling that applies to an
explicitly configured value. Documenting a small default (for example 8 KiB with
a 64 KiB maximum) is wrong and inverts the intended behavior.

## Rationale and evidence

- `edgion-gateway/src/ai/guards/request/spans.rs` — `select_spans`: `role`
  uses `string_token().ok_or(SpanError::Unparseable)` and returns `Unparseable`
  on `Field::Missing | Field::Ambiguous`; `content` returns `Unparseable` only on
  `Field::Ambiguous` and `continue`s on `Field::Missing`; the trailing
  `_ => continue` skips unsupported content types; part-level `type`/`text`
  return `Unparseable` only on `Field::Ambiguous`.
- `edgion-resources/src/resources/edgion_plugins/plugin_configs/ai_guard.rs` —
  `AI_GUARD_MAX_SPAN_BYTES` / `AI_GUARD_MAX_TOTAL_SPAN_BYTES` are `8 * 1024 * 1024`;
  the `max_span_bytes` doc comment records the deliberate absence of a small
  default, because a guard bound below what `AiProxy` accepts would turn ordinary
  long-context traffic into a content-policy refusal.
- `edgion-gateway/src/ai/guards/request/plugin.rs` — both caps resolve via
  `unwrap_or(body_cap)` from `request_body_config.max_body_size_bytes`.
- `edgion-resources/src/resources/edgion_gateway_config.rs` —
  `RequestBodyConfig::default_max_body_size` is `"32MiB"`; the parse-failure
  fallback `DEFAULT_MAX_BODY_SIZE_BYTES` in
  `edgion-gateway/src/request_body/config.rs` is the same 32 MiB. An operator may
  raise or lower the cap through `requestBody.maxBodySize`.

## Alternatives and limits

Trigger: the Chinese guide collapsed every missing/duplicate/ambiguous `role` or
`content` case into "unguardable" and documented an 8 KiB/64 KiB `maxSpanBytes`
pair. Root cause was translation drift, not a code defect — the English guide
already matched the source. Fixed by restating both semantics in
`docs/zh-CN/.../ai-guard.md`. This page records documentation parity for these
two behaviors only; it does not freeze the ordering contract, which is owned by
[AiGuard ordering errors fail closed at publication](ai-guard-ordering-publication.md).

## Verification and re-evaluation

Verified on 2026-09-23 by reading the cited sources and the existing `spans.rs`
unit tests (`ambiguous_role_or_content_is_unparseable`,
`mixed_non_object_parts_are_skipped_without_hiding_valid_text`, the duplicate
`type`/`text` cases). No new test was added: the change is documentation only.
Reconsider if span selection starts rejecting a currently skipped shape, if
either span cap gains a concrete default, or if the default request-body cap
changes — each requires updating both language guides together.

## Resolution references

- `align-ai-guard-chinese-body-semantics` (docs task, created 2026-09-10, closed
  2026-09-23) — pre-existing translation drift surfaced by the D0082 independent
  review; closed as documentation-only.
