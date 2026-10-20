# Webhook extraction path preparation

Status: deferred for user decision. No runtime change.
Source baseline: `116ca8eb5`.

## Evidence and decision

The Body branch of Gateway extract_value_from_response splits the configured
path on dots and attempts parse::<usize>() on every visited segment for each
successful webhook response. Both operations borrow the existing path; there
is no segment String allocation to remove. WebhookKeyGetConfig validates fields
but its validate_and_compile currently prepares no extraction state.

Compare the current traversal with immutable prepared steps: array indices and
key ranges into the owning path, or separately owned key strings. Preparation
could remove repeated delimiter scanning and integer parsing, but adds retained
metadata, construction work and potentially collection/key allocations. Account
for shallow paths, early lookup failures, plugin copies and overlapping config
generations. Full JSON parsing and the outbound exchange may dominate this work;
no benchmark establishes an end-to-end benefit yet. Keeping the current code is
a valid outcome.

The extraction plan belongs to the consumer's WebhookKeyGet configuration
generation. Different vars can reference the same LinkSys provider while using
different extraction paths. Do not put consumer-specific steps into a provider
client cache. Any range representation must retain the exact corresponding path
owner across moves, clones and reloads without self-referential borrows, unsafe
lifetime extension or a second global lookup. Decide the runtime representation
and exact cross-crate scope before implementation; this task adds no schema or
transport field.

This is separate from [selective JSON projection](webhook-01-selective-json-projection.md),
which changes document construction. It is also separate from
[claim path preparation](auth-03-claim-path-preparation.md): claim traversal uses
object keys even for numeric segments, whereas webhook traversal tries array
indices first. Shared machinery must not silently unify those policies.

## Required acceptance

- Preserve Rust usize parsing on the executing target, including accepted 00 and
  +0 spellings. A successful parse selects only an array index, with no fallback
  to a numeric object key. A parse failure, including integer overflow, selects
  the original string key. Do not replace this with a digits-only grammar or
  serialize platform-sized prepared indices across architectures.
- Preserve case-sensitive keys, permitted UTF-8, leading/trailing dots and
  consecutive dots. An empty whole path is rejected by current field validation;
  the direct extraction helper still treats it as one empty key. Keep admission
  and helper behavior distinct; add no new path validation, escaping, JSON
  Pointer semantics or root-selection shorthand.
- Keep complete JSON deserialization before lookup, including duplicate-key and
  invalid-unselected-input behavior. Missing paths and null yield None; selected
  strings transfer from the disposable parsed tree and other values serialize
  as before. Header, Cookie and BodyText extraction remain unchanged.
- Preserve bounded outbound acquisition and classification, per-var execution
  order, diagnostics, default/skip behavior and context writes. This permits no
  response cache, main-response aggregation, new logs or metrics, or outbound
  calls through local KeyGet accessors.
- Differentially compare prepared and existing traversal for nested arrays and
  objects, numeric/overflow/empty segments, missing and wrong-type branches,
  null and composite outputs. Cover two consumers sharing a provider with
  different paths and old/new consumer generations during reload. Measure full
  extraction cost and retained plan storage before selecting a representation.

## Sources and verification limits

- Gateway link_sys/providers/webhook/resolver.rs: extract_value_from_response.
- Gateway plugins/http/webhook_key_get/plugin.rs: create and ordered run_request.
- Resources resources/edgion_plugins/plugin_configs/webhook_key_get.rs:
  WebhookExtract, WebhookVar and field validation.
- skills/01-architecture/03-controller-gateway-link/webhook.md.
- skills/04-review/plugin-metadata/mandatory-design/keyget-is-local-io-free.md.

This is source-backed deferred work. No prototype, runtime test or performance
measurement was run for this documentation-only task. Future implementation
requires IMPL plan/final review and fresh behavioral verification.
