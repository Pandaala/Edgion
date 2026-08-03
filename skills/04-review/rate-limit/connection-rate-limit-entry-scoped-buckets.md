---
name: connection-rate-limit-entry-scoped-buckets
description: Use when reviewing stream ConnectionRateLimit bucket-identity findings; covers why its Redis keys and local token bucket are namespaced by the owning EdgionStreamPlugins entry.
status: fixed
finding_id: gw-plugin-acl-rl-04-stream
closed: 2026-08-03
---

# ConnectionRateLimit counters must be scoped to the owning plugin entry — FIXED

## Conclusion

The stream `ConnectionRateLimit` plugin identified its counters by listener port alone:
Redis keys were `{keyPrefix}conn:{algo}:0:lst:{port}` / `…:ip:{port}:{client_ip}` with `keyPrefix`
defaulting to `edgion:crl:` for every instance, and the Stage-1 local token bucket lived in a
process-global `HashMap<u16, Arc<LocalRateGate>>` keyed by port. Two `ConnectionRateLimit` entries
active on one port therefore shared both. Both are now namespaced by the owning entry's identity
(`<namespace>/<name>/#<index>`, from the shared `plugins::common::entry_scope`): the plugin folds it
into a `scoped_key_prefix`, and the gate registry is keyed by scope first, port second.

Re-reports of the "two limiters on one port share a bucket" form are **not accepted**; verify
`scoped_key_prefix` and the `LOCAL_GATES` key shape first.

## Core Rationale

**1. Two instances on one port are ordinary topology, not a corner case**

Stream plugins attach by annotation to an `EdgionStreamPlugins` store key at two levels — listener
level (`apply_connection_filter`, from the Gateway annotation) and route level
(`resolve_stream_plugin_store_key`, from each TCPRoute/TLSRoute's own annotation, which explicitly
accepts a cross-namespace key). Three reachable shapes: two TLSRoutes with different SNIs on one
port referencing different resources (`TlsRouteTable` is per-port, keyed by hostname); a
listener-level and a route-level attachment on the same connection; and two `ConnectionRateLimit`
entries inside one resource (`from_stream_plugins` instantiates every entry, and neither it nor the
controller preparse dedups by type).

**2. The consequence is worse than merged quotas**

`rate` and `window_ms` are Lua ARGV, never key material, and `policy_idx` is always 0 (the plugin
builds a one-element policy list), so two instances with the same algorithm index the identical key
and each compares the shared counter against its own limit — the tighter one silently governs both.
With differing `interval` it goes further: `window_start = now - (now % window)` is computed per
instance, so each sees the other's timestamp as stale and forces a window rotation on the wrong
boundary, losing and re-weighting counts in both directions.

**3. The local gate was the same bug without Redis**

`get_or_update_local_gate` is called on every connection and calls `update_params(rate, interval)`
on the slot it finds. Keyed by port alone, two instances rewrote each other's rate and interval on
every single connection — in the pure-local `perListener` mode, with no Redis configured at all.
Scoping the registry fixes that; keying by scope first (then port) also keeps the per-connection
lookup allocation-free, since `&str` borrows against the `String` key. The gate is keyed by the
entry scope alone, **not** by the Redis-prefixed form: the local bucket is a Redis-free backstop, so
editing `keyPrefix` — meaningless in pure `perListener` mode — must not move its registry slot and
reset the tokens. The registry has no eviction, so a rename / reorder / recreate orphans one small
slot for the process lifetime; accepted rather than plumbing a removal hook from the store.

**4. Identity, not configuration, is the discriminator**

Same reasoning as the HTTP sibling
([multi-source-key-composition-injective.md](multi-source-key-composition-injective.md), rationale 5):
the compiled runtime is built once per resource and Arc-shared, so the resource key is the sharing
unit, and it is derived from conf_sync so every Gateway instance and every reload agrees.
`StreamPluginEntry` has no `alias` field, so the scope always takes the `#<index>` branch —
reordering entries therefore restarts their counters.

## Fix Suggestions Not Accepted

- Namespace by listener port only (the previous state) — the port is not the sharing unit; the
  resource entry is.
- Ask operators to set a distinct `keyPrefix` per resource — the default is the failure mode, and
  nothing surfaces the collision when they do not.
- Give each plugin instance its own local gate field instead of the scoped global registry — it
  would drop the token state on every hot reload, which the registry exists to preserve.
- Key the registry by a `(String, u16)` tuple — forces an allocation per connection on the lookup
  path; the nested map borrows `&str` instead.

## Re-evaluation Triggers

Re-open this decision only if:

- `StreamPluginEntry` gains an alias (the scope should then use it, like the HTTP entries do).
- A stream plugin other than `ConnectionRateLimit` starts writing to a shared external store.

## Residuals NOT covered by this fix

Both are namespace-independent and remain open (see `tasks/todo/`):

- One resource attached at **both** listener and route level charges a single connection twice
  against the same scope. That is an evaluation-count question, not a bucket-identity one.
- Nothing rejects two `ConnectionRateLimit` entries in one `EdgionStreamPlugins`; they now get
  distinct scopes (so no corruption), but the connection is still charged twice.

## Reference Cases

- Finding `gw-plugin-acl-rl-04-stream` (2026-08-03), found while reviewing the HTTP sibling.
- Source: `edgion-gateway/src/plugins/stream/connection_rate_limit/{plugin.rs,local_gate.rs}`,
  `edgion-gateway/src/plugins/stream/{stream_plugin_runtime.rs,stream_plugin_store.rs}`,
  `edgion-gateway/src/plugins/common/mod.rs` (`entry_scope`).
- User docs: `docs/{en,zh-CN}/user-guide/tcp-route/stream-plugins.md` ("Bucket scope").
