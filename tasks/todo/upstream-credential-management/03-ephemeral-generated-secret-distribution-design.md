# Ephemeral Generated Secret Distribution Design

## Status

Design complete. Implementation has not started. Spec accepted 2026-08-27.

## Review findings (folded into this revision)

These are real gaps in the previous draft, not style nits.

1. **LinkSys is already a Gateway conf_sync kind.** Gateway dispatch is a
   no-op for `CredentialSource`, but the **whole LinkSys JSON still ships**.
   A HashMap field on `LinkSys` / `CredentialSourceConfig` would send every
   memory key to every Gateway. The map must live only on the Controller
   CredentialManager slot. It must not be a serde field, must not enter
   `save_to_cache`, Admin `/configclient` LinkSys output, or CRD status.
2. **`managed_credentials_supported()` is a single boolean** that currently
   rejects all `credentialSource` outside Kubernetes. Memory mode on FS/etcd
   needs a split: persist-on still requires Kubernetes; persist-off is
   allowed on a single-writer center.
3. **`ha_mode` is process topology, not a hot-reloaded resource field.**
   Do not invent a live requeue-all on ha_mode change. A mode switch is a
   controller restart, which rebuilds caches. The gate is: at parse time,
   persist-off plus current `ha_mode == all-serve` → `Accepted=False`.
4. **`memoryMaxGenerations: 1..=4` was the wrong knob.** OAuth2 still only
   keeps active (+ previous) publications. The HashMap needs a **DoS /
   future-provider ceiling** per LinkSys, default **10_000 keys**, and that
   ceiling is independent of the OAuth working set of at most two entries.
5. **Worst-case memory.** Each value is still bounded by the injector limit
   (~16 KiB). `10_000 × 16 KiB ≈ 160 MiB` per LinkSys is the operator-chosen
   ceiling. The 10_000 cap cannot be raised. Lowering it is allowed.
6. **Persist-off still embeds the active token on EdgionPlugins conf_sync.**
   That is the Gateway delivery path. It is not the same as syncing the
   LinkSys key map. Tests must prove both: LinkSys projection has zero
   generated keys; EdgionPlugins still carries `resolvedCredential` (Admin
   redacted).
7. **Persist `false` → `true` must not drop the memory slot before every
   current consumer Secret is consistent.** Partial fan-out is already
   valid in Task 2; clearing memory early creates a hole.

## Decision summary

Task 3 adds opt-in in-memory publication on `credentialSource` LinkSys.
Default remains Task 2's managed Secret path. Task 1 injection and Task 2
OAuth2 / `credentialRef` YAML stay unchanged.

- `publication.persist` defaults to `true`.
- `publication.persist: false`: no generated Secret. The leader holds a
  bounded key map for **that** LinkSys. Consumers still get
  `resolvedCredential.value` through EdgionPlugins conf_sync.
- That key map is **Controller-only**. LinkSys conf_sync, CRD, status, and
  Admin LinkSys views never contain map keys or values.
- `publication.memoryMaxKeys` defaults to `10000`, range `1..=10000`.
- Kubernetes `all-serve` plus `persist: false` is `Accepted=False`.
- No dedicated secret gRPC service. No Gateway generated-secret registry.
- Refresh cadence is unchanged.

## Relationship to Tasks 1 and 2

| Concern | Owner after Task 3 |
|---------|--------------------|
| Header target, order, mirror isolation, retry pinning | Task 1, unchanged |
| OAuth2 Client Credentials, bootstrap Secrets, rotation math, singleflight | Task 2, unchanged |
| Kubernetes Secret publication | Default. Skipped only when `persist: false` |
| Per-consumer same-name output Secret | Default. Not created in memory mode |
| `validUntil` Gateway enforcement | Task 2, unchanged |
| Plugin YAML `credentialRef` | Unchanged |
| LinkSys → Gateway conf_sync | Public spec only. No generated map |

```text
persist: true  (default)
  Leader acquire → per-consumer Secret → watcher requeue
    → embed token in EdgionPlugins → conf_sync → PluginRuntime

persist: false
  Leader acquire → per-LinkSys Controller memory map (≤ memoryMaxKeys)
    → enqueue consumers → embed *active* token in EdgionPlugins → conf_sync
  LinkSys conf_sync stays metadata-only (no map keys, no values)
```

Static core-Secret CredentialInjector entries are out of scope.

## Public schema

```yaml
apiVersion: edgion.io/v1
kind: LinkSys
metadata:
  name: provider-a-oauth
spec:
  type: credentialSource
  config:
    provider:
      type: oauth2ClientCredentials
      tokenEndpoint: https://auth.example.com/oauth2/token
      clientAuthentication:
        method: clientSecretBasic
        activeSecretRef:
          name: provider-a-oauth-client
    rotation:
      interval: 15m
    publication:
      persist: true              # default
      memoryMaxKeys: 10000       # default 10000; only used when persist is false
```

```rust
pub const MEMORY_MAX_KEYS_DEFAULT: u32 = 10_000;
pub const MEMORY_MAX_KEYS_MIN: u32 = 1;
pub const MEMORY_MAX_KEYS_MAX: u32 = 10_000;

#[derive(Clone, Debug, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CredentialPublicationConfig {
    #[serde(default = "default_persist")]
    pub persist: bool,

    #[serde(default = "default_memory_max_keys")]
    pub memory_max_keys: u32,
}
```

Omitted `publication` is persist-on, `memoryMaxKeys: 10000`.
`deny_unknown_fields` stays on.

Validation:

- `persist` is a YAML/JSON boolean. No string tri-state.
- `memoryMaxKeys` is an integer `1..=10000`. Out of range is
  `Accepted=False` even when `persist: true`, so a typo is not ignored.
  The cap **cannot be raised above 10000**.
- When `persist: true`, the cap is stored but unused.
- **The key map is not a spec field.** It must not appear on
  `CredentialSourceConfig`, `LinkSys`, or `LinkSysStatus`.

### Center and HA gates

| Center | `persist: true` | `persist: false` |
|--------|-----------------|------------------|
| Kubernetes `leader-only` | Task 2 Secret path | Controller memory map |
| Kubernetes `all-serve` | Task 2 Secret path | `Accepted=False`, `UnsupportedHaMode` |
| FS / etcd | `Accepted=False`, `UnsupportedConfigurationCenter` | Controller memory map |

Replace the blanket `managed_credentials_supported()` reject with:

- persist-on requires Kubernetes (`managed_secrets_supported`);
- persist-off requires a single publisher (`!all-serve`, including FS/etcd).

Expose `ha_mode` (or `single_publisher: bool`) on `ControllerRuntime` the
same way the Kubernetes flag is exposed today. Parse reads it; there is no
ha_mode watch.

Lenient admission still caches the object. CredentialManager must not
acquire or publish for an `Accepted=False` source. Consumers stay
`ResolvedRefs=False`.

## Per-LinkSys memory map (Controller-only)

Owned by the leader CredentialManager, keyed by LinkSys identity
`namespace/name/uid`. This **is** “the HashMap on that LinkSys”. It is not
a field of the resource object.

```rust
pub struct GeneratedSecretEntry {
    pub key: String,             // OAuth: publicationId; future providers: their key
    pub value: zeroize::Zeroizing<Vec<u8>>,
    pub issued_at: String,       // RFC3339 UTC seconds
    pub valid_until: String,     // RFC3339 UTC seconds
    pub valid_until_mono: Instant,
}

pub struct SourceMemorySlot {
    pub entries: IndexMap<String, Arc<GeneratedSecretEntry>>,
    pub active_key: String,
    pub max_keys: u32,
}
```

### Isolation from Gateway LinkSys sync

Hard rule: a generated entry is never serialized as part of a LinkSys.

| Surface | Generated keys/values |
|---------|------------------------|
| CRD / etcd / FS LinkSys document | Absent (`#[schemars(skip)]` is not enough — do not put the field on the struct) |
| Controller `save_to_cache` / conf_sync `kind=LinkSys` | Absent. Add a unit test that round-trips a memory-published LinkSys and finds no token-sized bytes and no `generated` / `memoryKeys` / `publicationId` value map |
| Admin `/configclient` LinkSys | Same projection as conf_sync. Metadata-only |
| Gateway `SystemConfig::CredentialSource` | No-op dispatch **and** no map to parse. Forged unknown fields stay ignored (`deny_unknown_fields` on the config struct) |
| EdgionPlugins `resolvedCredential` | **Allowed** — this is the one Gateway delivery path, same as Task 2 after Secret resolve. Admin redacts `value` |

`ControllerCredentialSourceConfig` already exists to keep resolved TLS off
the wire. Do not weaken it by cloning the memory slot into `manager_config`
or back onto `ls.spec`.

### Caps and eviction

Two layers:

| Layer | Bound | Purpose |
|-------|--------|---------|
| Operator `memoryMaxKeys` | default 10000, max 10000 | HashMap size / DoS ceiling per LinkSys |
| OAuth2 working set | at most 2 entries | `active` plus previous `publicationId` |

OAuth2 insert:

1. New publication becomes `active_key`.
2. Keep the previous active as a non-active entry.
3. Drop any additional older OAuth publication ids immediately (working
   set 2), even if `memoryMaxKeys` is 10000. Do **not** retain thousands
   of historical tokens.
4. If `entries.len() > max_keys` after that (a future many-key provider),
   drop oldest non-active first. Never drop `active_key` to satisfy the
   cap. If there is no non-active victim, **reject the insert**
   (fail-closed, keep the previous active). Do not evict a live key.

Lookup for EdgionPlugins publish uses **only** `active_key`. In-flight
Gateway requests keep Task 1's captured `Arc<HeaderValue>`.

`Debug` prints keys, timestamps, counts — never `value`.
Store-stats: `credential_memory_keys` (sum of lengths) and
`credential_memory_sources`. Keys reported to residue detection are
LinkSys identities, not secret keys, not values.
Leadership loss, runtime reset, source delete/type change, last consumer
gone, and wipe-all clear the slot.

## Controller flow

Workqueue discipline is unchanged.

Publish, fenced on the **current** persist flag:

1. Recheck leader fence, source UID, generation, `Accepted`, persist, and
   single-publisher.
2. `persist: true`: existing Secret fan-out.
3. `persist: false`: insert into `SourceMemorySlot`. Do not create or
   patch a generated Secret. Enqueue current consumers.
4. Serial LinkSys worker writes metadata status only.

EdgionPlugins LinkSys resolution:

- `persist: true`: managed Secret path (register same-name Secret dep).
- `persist: false`: **do not** register the same-name Secret dependency.
  Non-blocking snapshot of `active_key`. Copy that one value into
  `ResolvedCredentialInjector`. Do not copy the rest of the map onto the
  plugin or the LinkSys.

### Persist mode switches

| Switch | Action |
|--------|--------|
| `true` → `false` | Copy the current token into the memory slot (reacquire if the manager has no live token). Then enqueue owned-Secret cleanup with Task 2 UID / binding fences. |
| `false` → `true` | Fan out the active memory entry to consumer Secrets. **Keep the memory slot until every current consumer Secret is consistent.** Then clear the slot. |
| Stay `false`, `memoryMaxKeys` shrinks | Evict oldest non-active until `len <= max_keys`. If still over cap, fail-closed (cannot drop active). |

## Gateway

No new Gateway registry.

LinkSys ConfHandler: `CredentialSource` remains a no-op. It must not grow
a secret table.

EdgionPlugins still receives one embedded `{ value, validUntil }` for the
active generation. `validUntil` → monotonic `Instant` at runtime build;
checked at injector run and each upstream attempt.

## Refresh cadence (unchanged)

```text
next = min(expires_in / 2, rotation.interval) − earlier-only jitter
```

Default interval 15m. Memory mode uses the same scheduler. Each acquire
enqueues consumers (EdgionPlugins embed), not a LinkSys-body update.

## HA, restart, reconnect

| Mode | Token durability | Follower | Leader restart |
|------|------------------|----------|----------------|
| `persist: true` | Managed Secret | Watch Secret, embed | Task 2 adopt or reacquire |
| `persist: false` | Leader memory map | Not a publisher (`all-serve` rejected) | Empty map, always reacquire |

Memory-mode failover: old map is dropped; Gateways keep the last
PluginRuntime until `validUntil`; new leader reacquires; no reading tokens
back from Gateway or from LinkSys snapshots.

## Threat model

### Default (`persist: true`)

Task 2. Generated token is a Kubernetes Secret plus EdgionPlugins embed.
Not non-persistent.

### Memory mode (`persist: false`)

**No intentional durable generated Secret.** Also **no generated key map
on LinkSys transport.**

| Surface | Rule |
|---------|------|
| Generated Kubernetes Secret | Not created; leftovers swept |
| FS / etcd LinkSys document | No map |
| LinkSys conf_sync / Admin LinkSys | No map keys, no values |
| LinkSys / EdgionPlugins status | Metadata only |
| Logs / metrics / PluginLog | No value |
| EdgionPlugins conf_sync | Active value only, same as Task 2; Admin redacts |
| Process memory, swap, coredump | Out of guarantee |

Do not say "never touches disk" or "Gateway never sees the token".
Gateway sees the **active** token through EdgionPlugins, not through
LinkSys.

## Status and observability

No new condition types.

- `UnsupportedConfigurationCenter` — persist-on outside Kubernetes
- `UnsupportedHaMode` — persist-off plus `all-serve`

Optional scalar `publicationPersist: true|false`. Never a token hash,
never a key listing of the memory map.

## Failure policy

Same generic fail-closed injector as Task 1/2. No new fail-open knob.
Missing/expired active generation or rejected HA → unresolved, not "keep
the last Secret".

## Testing

1. Schema: default persist; `persist: false`; `memoryMaxKeys` default
   10000; `0` and `10001` rejected; unknown `publication` keys rejected.
2. **LinkSys isolation:** after a memory publish, serialize the cached
   LinkSys (conf_sync / Admin projection). Assert no generated map, no
   token bytes, no extra keys beyond the public spec. Gateway fixture
   deserializes that payload into `CredentialSource` and remains a no-op.
3. Persist-on: existing Secret tests still pass.
4. Persist-off, Kubernetes leader-only: no Secret created; plugin embed
   still works; OAuth working set stays ≤ 2 entries even when
   `memoryMaxKeys` is 10000; last consumer clears the slot; residue zero.
5. Cap: fill to `memoryMaxKeys` with non-active extras (test helper);
   insert rejects or evicts per the rule; `active_key` never dropped.
6. `all-serve` + persist-off: `Accepted=False`, no issuer call.
7. FS/etcd + persist-off: acquire without Secret I/O. persist-on: still
   unsupported.
8. Mode switch: `true` → `false` deletes Secrets; `false` → `true` keeps
   memory until fan-out completes; delayed delete cannot remove the new
   Secret.
9. Gateway injector tests unchanged. k8s case: persist-off leaves no
   Secret. Do not snapshot `resolvedCredential.value`.

## Module map

| Path | Responsibility |
|------|----------------|
| `edgion-resources/.../credential_source.rs` | `publication` spec, `memoryMaxKeys` |
| `edgion-controller/.../credential_manager/` | `SourceMemorySlot`, persist branch, **never write the slot onto LinkSys** |
| `edgion-controller/.../handlers/link_sys.rs` | Split K8s-secret vs single-publisher gates; keep clearing resolved TLS; do not attach the map |
| `edgion-controller/.../runtime.rs` | `single_publisher` / ha_mode flag |
| `edgion-controller/.../credential_injector.rs` | Memory snapshot → plugin embed only |
| Gateway LinkSys store | Keep `CredentialSource` no-op; add a test that a map-shaped forged field cannot be stored |
| store-stats | Memory source/key counts |
| `edgion-tests` | Isolation, cap, HA reject, mode switch, residue |

## Non-goals

- Syncing the memory map through LinkSys or a new RPC.
- Per-consumer copies of the same token in the map.
- Raising `memoryMaxKeys` above 10000.
- Moving static Secret consumers onto the memory path.
- Gateway OAuth / Vault / STS.
- Cookie injection / `credentialPool` changes.
- `mlock`.

## Original Task 3 requirements this design revises

- Persistence is a LinkSys switch, default on.
- Memory keys stay on the Controller slot and are **not** LinkSys-synced.
- The active value still travels on EdgionPlugins conf_sync.
- Dedicated secret gRPC is out.
