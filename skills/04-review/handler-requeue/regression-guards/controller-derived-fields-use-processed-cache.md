---
name: controller-derived-fields-use-processed-cache
description: Use when reviewing cross-kind Controller lookups that consume fields populated by preparse or parse; covers why those reads must use the processed ServerCache view instead of RawStore.
status: fixed
finding_id: ctrl-04
closed: 2026-08-09
knowledge_type: regression-guards
---

# Controller-derived Fields Use the Processed Cache

## Conclusion

A cross-kind decision that depends on Controller-derived fields must read the
processed `ServerCache` object through `ProcessorObj::get_cached_any`. RawStore
is the correct source for unfiltered input and `on_change` self-lookups, but it
cannot expose fields produced by `preparse` or `parse`.

For HTTPRoute AI admission, `ai_proxy_admission_state` reads the cached
EdgionPlugins and keeps three meanings distinct: an unstamped enabled AiProxy
is active, Bypass removes it from the active count, and Block preserves the
rule so the Gateway can compile its unconditional `PluginPolicyBlock` gate.
Because Gateway promotes a Block from any enabled HTTP stage entry to that
request-stage gate, the Controller scans the entire referenced plugin chain,
not only AiProxy entries.

## Core Rationale

**1. RawStore and ServerCache represent different lifecycle stages**

RawStore holds the resource as received from Kubernetes, FileSystem, or etcd.
ServerCache holds the handler-filtered object after validation, PluginPolicy
stamping, reference resolution, and status processing; that processed object is
the snapshot sent through conf_sync.

**2. PluginPolicy stamps are Controller-owned**

Inbound `policy_action` values are cleared and recomputed during EdgionPlugins
preparse. Reading RawStore therefore treats both Block- and Bypass-stamped
AiProxy entries as active. The processed view is also required to distinguish
their enforcement: Bypass skips the plugin and continues, while Block replaces
it with a fail-close request gate. Once any referenced chain contains Block,
that gate takes precedence over every AI-specific admission return in
`ai_rule_invalid_reason`, including attachment-shape errors and a sibling
missing EdgionPlugins reference; otherwise dropping the rule could let another
rule handle the request.

**3. One lookup must choose one coherent view**

Checking existence in ServerCache and then reading content from RawStore can
combine different generations and filter semantics. A single
`get_cached_any` lookup makes availability and content come from the snapshot
the Gateway consumes.

## Fix Suggestions Not Accepted

- Check `policy_action` directly on the RawStore object. Inbound values are
  untrusted and the canonical stamp has not run there.
- Treat every non-`None` action as a missing AiProxy. That is correct for
  Bypass but would drop the route before the Gateway can compile a Block gate,
  potentially allowing another route rule to handle the request.
- Inspect Block only on AiProxy or only in `requestPlugins`. Gateway promotes
  Block from every enabled entry in all four HTTP stages to the same
  unconditional request gate.
- Clone and re-run PluginPolicy stamping at every caller. That duplicates the
  resource pipeline and can still diverge from other parsed fields.
- Serialize all cached resources and search the JSON snapshot. Processed
  resources may contain resolved secrets, and typed lookup avoids both exposure
  risk and serialization cost.

## Re-evaluation Triggers

Revisit only if Controller-derived fields move into the raw ingest stage or if
the processor pipeline publishes an immutable typed snapshot interface that
replaces `get_cached_any`.

## Reference Cases

- `ctrl-04` — HTTPRoute AI admission ignored PluginPolicy-stamped AiProxy
  entries.
- `edgion-controller/src/conf_mgr/sync_runtime/resource_processor/processor.rs`
  (`ProcessorObj::get_raw_any`, `get_cached_any`).
- `edgion-controller/src/conf_mgr/sync_runtime/resource_processor/handlers/http_route.rs`
  (`ai_proxy_admission_state`,
  `ai_proxy_admission_uses_policy_stamped_cache_and_preserves_block_gate`,
  `plugin_policy_block_precedes_every_ai_admission_drop`).
- Architecture: `skills/01-architecture/01-controller/05-resource-processor.md`.

## Resolution references

- `ctrl-04` — 2026-08-09.
