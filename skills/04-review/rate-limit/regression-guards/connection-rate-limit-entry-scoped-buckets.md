---
name: connection-rate-limit-entry-scoped-buckets
description: Use when reviewing stream ConnectionRateLimit bucket-identity findings; covers why its Redis keys and local token bucket are namespaced by the owning EdgionStreamPlugins entry.
status: fixed
finding_id: gw-plugin-acl-rl-04-stream
closed: 2026-08-03
knowledge_type: regression-guards
---

# ConnectionRateLimit counters must be scoped to the owning plugin entry — FIXED

## Conclusion

The stream `ConnectionRateLimit` plugin identified its counters by listener port alone:
Redis keys were `{keyPrefix}conn:{algo}:0:lst:{port}` / `…:ip:{port}:{remote_ip}` with `keyPrefix`
defaulting to `edgion:crl:` for every instance, and the Stage-1 local token bucket lived in a
process-global `HashMap<u16, Arc<LocalRateGate>>` keyed by port. Two `ConnectionRateLimit` entries
active on one port therefore shared both. Both are now namespaced by the owning entry's identity
(`<namespace>/<name>/#<index>`, from the shared `plugins::common::entry_scope`): the plugin folds it
into a `scoped_key_prefix`, and the gate registry is keyed by scope first, port second.

For a new sharing report, verify `scoped_key_prefix`, `LocalGateBinding` and the
entry/port registry identity against current callers. Identity isolation alone
does not establish the concurrency or configuration-transition contract; see the
[local admission guard](local-connection-admission-transitions.md).

## Core Rationale

**1. Two instances on one port are ordinary topology, not a corner case**

Stream plugins attach by annotation to an `EdgionStreamPlugins` store key at two levels — listener
level (`apply_connection_filter`, from the Gateway annotation) and route level
(`resolve_stream_plugin_store_key`, from TCPRoute's annotation, which accepts a
cross-namespace key). A listener-level and a TCPRoute-level attachment can apply
to the same connection; multiple enabled `ConnectionRateLimit` entries inside
one resource also run independently. Historical TLSRoute examples are no longer
applicable: TLSRoute now uses its separate Stage-2 runtime, not this limiter.

**2. The consequence is worse than merged quotas**

`rate` and `window_ms` are Lua ARGV, never key material, and `policy_idx` is always 0 (the plugin
builds a one-element policy list), so two instances with the same algorithm index the identical key
and each compares the shared counter against its own limit — the tighter one silently governs both.
With differing `interval` it goes further: `window_start = now - (now % window)` is computed per
instance, so each sees the other's timestamp as stale and forces a window rotation on the wrong
boundary, losing and re-weighting counts in both directions.

**3. The local gate was the same bug without Redis**

Historically `get_or_update_local_gate` applied parameters on every connection;
with a port-only key, different entries rewrote each other's configuration even
in pure-local mode. Entry scoping corrected that identity bug. D0100 separately
replaces per-hit updates with construction-bound generations and coherent local
admission; those are independent corrections.

The current local registry identity remains entry scope plus port, excluding the
Redis prefix. Editing `keyPrefix` must not reset a Redis-free local budget.
Bindings reference the persistent registry rather than owning disposable buckets.
No eviction is still the accepted lifetime choice: changed names/positions can
retain unused entries for the process lifetime, while reusing the same identity
finds its retained budget. Scope count can grow with operator churn; it is not
globally memory-bounded.

**4. Identity, not configuration, is the discriminator**

Same reasoning as the HTTP sibling
([multi-source-key-composition-injective.md](multi-source-key-composition-injective.md), rationale 5):
the compiled runtime is built once per resource and Arc-shared, so the resource key is the sharing
unit, and it is derived from conf_sync so every Gateway instance and every reload agrees.
`StreamPluginEntry` has no `alias` field, so the scope always takes the `#<index>`
branch. Reordering changes which identity a policy uses; an already used index
can retain a local budget, so reordering does not guarantee fresh counters.

## Fix Suggestions Not Accepted

- Namespace by listener port only (the previous state) — the port is not the sharing unit; the
  resource entry is.
- Ask operators to set a distinct `keyPrefix` per resource — the default is the failure mode, and
  nothing surfaces the collision when they do not.
- Give each runtime its own disposable bucket — it would drop token state on
  reload. A runtime field holding a binding to the persistent entry is correct.
- Construct owned scope/port lookup keys on each connection — resolve entry
  identity during construction and reuse its bounded port dimension.

## Re-evaluation Triggers

Re-evaluate when:

- `StreamPluginEntry` gains an alias (the scope should then use it, like the HTTP entries do).
- A stream plugin other than `ConnectionRateLimit` starts writing to a shared external store.
- Identity, registry lifetime or configuration-generation activation changes.

## Layered attachment semantics

Listener-level and TCPRoute attachments are cumulative policy layers. TLSRoute runs only its
Stage-2 chain; it no longer falls back to the Stage-1 connection-rate-limit runtime. UDP has no
Gateway-level `ConnectionFilter`. Multiple entries in one resource are independent budgets. The
exact same resource attached at both applicable levels does evaluate the same entry scope twice, but
this low-frequency configuration overlap is an accepted tradeoff; cross-stage hot-path
deduplication is not justified without an operator incident. See
[connection-rate-limit-layered-attachments.md](connection-rate-limit-layered-attachments.md).

## Reference Cases

- Finding `gw-plugin-acl-rl-04-stream` (2026-08-03), found while reviewing the HTTP sibling.
- Source: `edgion-gateway/src/plugins/stream/connection_rate_limit/{plugin.rs,local_gate.rs}`,
  `edgion-gateway/src/plugins/stream/{stream_plugin_runtime.rs,stream_plugin_store.rs}`,
  `edgion-gateway/src/plugins/common/mod.rs` (`entry_scope`).
- User docs: `docs/{en,zh-CN}/user-guide/tcp-route/stream-plugins.md` ("Bucket scope").

## Resolution references

- `gw-plugin-acl-rl-04-stream` — 2026-08-03.
- `D0100` — preserves this identity rule while correcting local admission
  transitions; current verification is recorded in the linked admission guard.

## Active Redis source identity

The resource factory checks the actual owner's identity before reusing a cached
ConnectionRateLimit entry when `needs_redis()` is active. This guard does not
change raw entry-scope counter keys, local-only behavior, provider failure
policy, captured-view lifetimes, or cumulative attachment layers. Configuration
is borrowed from the actual owner; a valid-looking raw resource key is not an
owner credential.
