---
name: controller-config
description: Full schema for the edgion-controller YAML configuration.
---

# Controller YAML Configuration Schema

> Default file path: `config/edgion-controller.yaml`, specified via `--config-file`.

## Files to Reference

- Sample configuration: `config/edgion-controller.yaml`
- Configuration structure: `edgion-controller/src/config/mod.rs`
- `conf_center` top level: `edgion-controller/src/config/conf_center/config.rs`
- FileSystem mode: `edgion-controller/src/config/conf_center/file_system.rs`
- Etcd mode: `edgion-controller/src/config/conf_center/etcd.rs`
- Kubernetes mode: `edgion-controller/src/config/conf_center/kubernetes.rs`

## Full Schema

```yaml
# Working directory (only fields that explicitly use the work-dir resolver use this base)
# Priority: CLI --work-dir > ENV EDGION_WORK_DIR > this value > "."
work_dir: "."

# Local admin token (Istio-style pre-shared bearer). Grants allow-all access to
# the Admin API. EDGION_ADMIN_TOKEN env var takes precedence; blank = not configured.
# Prefer the env var (from a K8s Secret) over an in-file value.
# admin_token: "edg_…"

server:
  grpc_listen: "0.0.0.0:12151"    # gRPC listen address (ConfigSyncServer)
  admin_listen: "127.0.0.1:12101"  # Admin HTTP API address (/api/v1, /configclient); default loopback
  # probe_listen: "0.0.0.0:12100"   # Liveness/readiness (/health, /ready); default 0.0.0.0:12100
  # metrics_listen: "0.0.0.0:12190" # Prometheus metrics (/metrics); default 0.0.0.0:12190
  # allow_admin_ips:                 # Optional IP allowlist for the Admin API listener; unset/empty = allow all
  #   - "10.0.0.0/8"
  #   - "127.0.0.1/32"
  #   - "::1/128"

logging:
  log_dir: logs                    # Log directory
  log_prefix: edgion-controller    # Log file prefix
  log_level: info                  # trace | debug | info | warn | error
                                   # Module-level filtering supported: info,edgion::core::controller=debug
  json_format: false               # JSON structured log
  console: true                    # Also output to console
  buffer_size: 10000               # Log buffer size

debug:
  enabled: true                    # Enable debug features

validation:
  enable_reference_grant_validation: true   # Enable cross-namespace ReferenceGrant validation
  # Optional plugin allowlist/denylist. Absent (default) = allow all plugins.
  # See "[validation.plugin_policy]" below for the full schema.
  # plugin_policy:
  #   mode: denylist
  #   plugins:
  #     - name: http/Wasm

conf_sync:
  default_capacity: 200            # EventStore default capacity (for all resource types)
  # Override capacity for specific resource types
  # Note: setting no_sync_kinds completely replaces the default list, it is not appended
  capacity_overrides:
    # HTTPRoute: 500
    # Service: 1000
  # no_sync_kinds: [ReferenceGrant, Secret]   # Resources not synced to Gateway

# ─── gRPC channel security (REQUIRED — omitting it refuses to start) ───
# conf_sync_security:
#   active: primary
#   certs:
#     - name: primary
#       cert: certs/grpc/server.crt    # Controller server certificate
#       key: certs/grpc/server.key     # Server private key
#       ca: certs/grpc/ca.crt          # CA used to verify Gateway client
#       skip_tls: false                # true = explicit plaintext opt-in (ERROR log + skip_tls metric)
#   auto_ca: false                   # Kubernetes only: internal CA auto-provisions certs
#   rotation:                        # Controller-only; consumed only when auto_ca=true
#     policy: lifetime                     # lifetime | scheduled | manual
#     lifetime_ratio: 0.5
#     # schedule: "0 3 * * *"               # required iff policy=scheduled (5-field cron)
#     schedule_timezone: "UTC"
#     rotate_if_remaining_below: "336h"
#     emergency_rotate_below: "24h"
#     leaf_ttl: "365d"
#     ca_ttl: "3650d"

# ─── FileSystem mode ───
conf_center:
  type: file_system
  conf_dir: config/resources              # YAML resource directory

# ─── Kubernetes mode ───
# conf_center:
#   type: kubernetes
#   gateway_class: edgion                 # Required: GatewayClass to match
#   controller_name: edgion.io/gateway-controller  # controllerName reported in Gateway API status
#   watch_namespaces: []                  # Empty = all namespaces
#   label_selector: ""                    # Resource label selector (top-level in this variant)
#   gateway_address: ""                   # Address reported in Gateway status
#   ha_mode: leader-only                  # "leader-only" | "all-serve"
#   no_watch_kinds: [TCPRoute, UDPRoute]  # Resource kinds to skip CRD precheck and watch for
#   cache_ready_deadline_secs: 90         # Pre-CachesReady deadline (secs); restart-bound, not hot-reloadable
#   leader_election:
#     lease_name: edgion-controller-leader
#     lease_namespace: edgion-system      # Default: POD_NAMESPACE, else "default"
#     lease_duration_secs: 15
#     renew_deadline_secs: 10
#     retry_period_secs: 2
#   metadata_filter:
#     blocked_annotations: []             # Annotations removed before entering memory (defaults remove common large kubectl/helm annotations)
#     remove_managed_fields: true         # Reduce object size

# ─── Etcd mode ───
# conf_center:
#   type: etcd
#   endpoints: ["https://etcd-0:2379", "https://etcd-1:2379"]
#   key_prefix: /edgion                   # Default: /edgion
#   connect_timeout_ms: 5000              # Optional
#   request_timeout_ms: 10000             # Optional
#   keep_alive_time_secs: 30              # Optional
#   keep_alive_timeout_secs: 10           # Optional
#   auth:                                 # Optional; this file must be Secret-mounted if used
#     username: edgion
#     password: "..."
#   tls:                                  # Optional
#     ca_cert_file: /etc/edgion/etcd/ca.pem
#     client_cert_file: /etc/edgion/etcd/client.pem
#     client_key_file: /etc/edgion/etcd/client-key.pem
```

## Field Details

### [server]

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `grpc_listen` | `String` | `0.0.0.0:12151` | ConfigSyncServer gRPC address |
| `admin_listen` | `String` | `127.0.0.1:12101` | Admin HTTP API address — serves `/api/v1/*` and `/configclient`. Defaults to loopback; set to `0.0.0.0` for cluster-wide access and pair with `allow_admin_ips` and/or a NetworkPolicy (see [Admin API network exposure](./03-auth-bootstrap.md#admin-api-network-exposure)). |
| `probe_listen` | `String` | `0.0.0.0:12100` | Probe listener — serves `/health` (liveness) and `/ready` (readiness) |
| `metrics_listen` | `String` | `0.0.0.0:12190` | Metrics listener — serves `/metrics` (Prometheus) |
| `allow_admin_ips` | `Vec<String>` | `[]` (allow all) | CIDR allowlist for the Admin API listener. **Unset or empty = allow all.** Matched against the **TCP peer address only**; `X-Forwarded-For` is never trusted, so it is meaningful only for direct connections to the admin socket (behind an L7 proxy it degrades to matching the proxy IP). Probe/metrics run on dedicated ports and are unaffected. Dual-stack: when the listener binds `::`, list CIDRs for both families; NAT64 `64:ff9b::/96` embedded IPv4 is not extracted. **Recommended whenever `admin_listen` is bound to a non-loopback address**; leaving it empty in that case triggers the `insecure_admin_exposure` gauge (see [Admin API network exposure](./03-auth-bootstrap.md#admin-api-network-exposure)). Example: `["10.0.0.0/8", "127.0.0.1/32", "::1/128"]` |

> **Admin API transport is plaintext HTTP.** The Controller Admin API listener is always served over plain HTTP — there is no `admin_tls` config surface. Transport security is delegated to the deployment environment: bind it to a private interface, restrict peers with `allow_admin_ips`, and rely on Kubernetes ClusterIP / NetworkPolicy. Authentication is still enforced independently via the Bearer token (see [auth bootstrap](./03-auth-bootstrap.md)); a request reaching a plaintext socket without a valid token is rejected.

### [logging]

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `log_dir` | `String` | `logs` | Log directory (relative to work_dir) |
| `log_prefix` | `String` | `edgion-controller` | Log file prefix |
| `log_level` | `String` | `info` | Log level, supports module-level filtering |
| `json_format` | `bool` | `false` | JSON structured output |
| `console` | `bool` | `true` | Also output to console |
| `buffer_size` | `usize` | `10000` | Async log buffer size |

> Note: `log_dir` is resolved via `work_dir().resolve()`, so a relative path typically lands in `work_dir/logs/...`.

### [conf_center]

The tagged enum accepts exactly `file_system`, `kubernetes`, or `etcd`.

- FileSystem `conf_dir` is resolved relative to the process current working directory,
  not `work_dir`; `--conf-dir` replaces the YAML FileSystem variant.
- Kubernetes `label_selector` is a direct field of the Kubernetes variant. It must not
  be nested under `metadata_filter`, whose only fields are `blocked_annotations` and
  `remove_managed_fields`.
- Kubernetes leader election is always enabled. `ha_mode` accepts `leader-only`
  (default: only the leader watches/processes/serves gRPC) or `all-serve` (all replicas
  watch/process/serve; coordinated writes remain leader-only).
- Etcd `auth.password` is raw secret material. Keep the configuration in a
  Secret-mounted file and use TLS for the etcd connection.

### [validation]

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `enable_reference_grant_validation` | `bool` | `true` | Enable ReferenceGrant validation; disabling lifts cross-namespace reference restrictions |
| `plugin_policy` | `PluginPolicyConfig?` | `None` (allow all) | Controller-side plugin allowlist/denylist. See [validation.plugin_policy](#validationplugin_policy) below. |

### [validation.plugin_policy]

**Source:** `edgion-controller/src/config/plugin_policy.rs` (schema, fail-fast validation, compiled lookup); field lives on `ValidationConfig` in `edgion-controller/src/config/mod.rs`.

Optional Controller-side allowlist/denylist that restricts which plugin
types the Gateway is allowed to instantiate. `plugin_policy` is absent by
default — every existing configuration is unaffected, no plugin is
stamped, no status condition is emitted, and the Gateway takes the
existing zero-overhead path. The feature only activates when an operator
explicitly adds this section. **Config-file only (no CLI flag); changes
require a Controller restart** — there is no hot reload for this section.

Full example (all fields shown):

```yaml
validation:
  plugin_policy:
    mode: denylist            # denylist | allowlist (required)
    default_action: block     # block | bypass (default: block)
    default_block_status: 503 # HTTP status for block (default: 503)
    plugins:
      - name: http/Wasm                 # shorthand: uses default_action
      - name: http/Dsl
        action: block
        block_status: 403               # per-entry status override
      - name: stream/ConnectionRateLimit
        action: bypass
```

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `mode` | `"denylist" \| "allowlist"` | **Required** | See "Semantics" below. |
| `default_action` | `"block" \| "bypass"` | `block` | Action applied to a restricted entry that has no per-entry `action`. |
| `default_block_status` | `u16` (400–599) | `503` | HTTP status returned by a restricted entry that has no per-entry `block_status`. Stream/TLS-route stages ignore this — a restricted connection is simply closed. |
| `plugins` | `Vec<{name, action?, block_status?}>` | `[]` | Qualified plugin names this policy applies to. In `allowlist` mode a listed entry means "allowed" and MUST NOT set `action`/`block_status` (startup error if it does). |

**Qualified names** — `<family>/<Type>`, exactly three families, exact
case-sensitive match against the serde tag, no wildcards:

| Family | Enum | Governs |
|--------|------|---------|
| `http` | `EdgionPlugin` | All four HTTP stages of `EdgionPlugins` (`requestPlugins`, `upstreamResponseFilterPlugins`, `upstreamResponseBodyFilterPlugins`, `upstreamResponsePlugins`) |
| `stream` | `EdgionStreamPlugin` | Connection stage (`spec.plugins`) of `EdgionStreamPlugins` |
| `tls-route` | `TlsRouteStreamPlugin` | TLS-route stage (`spec.tlsRoutePlugins`) of `EdgionStreamPlugins` |

Gateway API standard filter variants inside `EdgionPlugins`
(`RequestHeaderModifier`, `RequestRedirect`, `UrlRewrite`, ...) are ordinary
`EdgionPlugin` variants and are governed like any other `http/` plugin —
there is no exemption list. Allowlist users must list the standard filters
they use. The canonical name sets are extracted once at startup from
`schema_for!` of the three enums (the same technique as
`edgion-resources/tests/plugin_crd_type_coverage.rs`) — there is no
hand-maintained list to drift.

**Semantics:**

- Policy absent (default) → allow all: every plugin runs, unchanged behavior.
- `denylist` → listed plugins are restricted; everything else runs.
- `allowlist` → **global** across all three families: any plugin not
  listed, in any family, is restricted and handled with `default_action`.
  There is no per-family carve-out — forgetting to list a family fails
  safe (visible restriction), not silently-allow.
- Policy changes require a Controller restart.

**Fail-fast (Controller startup fails on):**

- unknown family or unknown plugin name;
- duplicate entries;
- `block_status` (default or per-entry) outside `400..=599`;
- `action` / `block_status` present on an allowlist-mode entry.

**Effect on a restricted plugin entry:** the Controller does not flip the
entry's `enable` field or block the resource from being cached/synced. It
stamps an internal `policy_action` marker (never operator-facing —
`#[schemars(skip)]`, not present in the CRD schema) and reports a
`PluginPolicyRestricted=True` status condition listing the hits;
`Accepted` is unaffected. Enforcement (blocking or bypassing the plugin at
request time) happens on the Gateway. See `docs/en/ops-guide/plugin-policy.md`
(`docs/zh-CN/ops-guide/plugin-policy.md` for the Chinese version) for the
full operator guide, including the mandatory upgrade-order warning before
enabling this feature on a fleet with mixed Gateway versions.

### [conf_sync]

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `default_capacity` | `u32` | `200` | EventStore default capacity |
| `capacity_overrides` | `Map<String, u32>` | `{}` | Per-kind capacity override |
| `no_sync_kinds` | `Vec<String>?` | `["ReferenceGrant", "Secret"]` | List of resources not synced to Gateway (when set, completely replaces the default) |

### conf_sync_security

**Required section**: use `auto_ca`, or select an entry from `certs`. When the
selected entry carries certificate paths, the Controller's gRPC port uses mTLS.

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `active` | `String?` | — | Name of the selected `certs` entry. Optional only when `certs` contains exactly one entry. |
| `certs` | `Vec<ConfSyncTlsConfig>` | `[]` | Named channel-security entries. This is the current file-mode schema. |
| `auto_ca` | `bool` | `false` | Kubernetes only: provision Controller and Gateway identities from the internal CA. |
| `rotation` | `ConfSyncRotationConfig?` | — | Controller-only auto-CA rotation policy. |

Each `certs` entry has:

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `name` | `String?` | — | Entry name; required when more than one entry exists. |
| `cert` | `String` | — | Server certificate path (PEM, relative to work_dir) |
| `key` | `String` | — | Server private key path (PEM) |
| `ca` | `String` | — | CA certificate path (used to verify the Gateway client certificate) |
| `skip_tls` | `bool` | `false` | Explicit plaintext opt-in: `true` skips TLS and starts in plaintext (ERROR log + `skip_tls` metric). The ONLY way to run this channel in plaintext. |

> **Removal debt:** code still accepts the old singular
> `conf_sync_security.tls` field. It is not a supported configuration contract;
> do not use it in new configuration or examples.

> **Fail-close (2026-07-14, BREAKING):** if `conf_sync_security` is **omitted entirely**,
> the Controller **refuses to start** (non-zero exit). The conf_sync channel carries
> resolved Secrets (TLS private keys, backend credentials) to the data plane, so plaintext
> must be an explicit, audited choice — never the consequence of a forgotten config block.
> The startup error names the three ways forward.
>
> Choose one current configuration:
>
> ```yaml
> # (a) Kubernetes: the Controller's internal CA provisions everything
> conf_sync_security:
>   auto_ca: true
>
> # (b) file mode: real mTLS from mounted certs
> conf_sync_security:
>   active: primary
>   certs:
>     - name: primary
>       cert: certs/grpc/server.crt
>       key: certs/grpc/server.key
>       ca: certs/grpc/ca.crt
>
> # (c) plaintext, explicitly (INSECURE)
> conf_sync_security:
>   active: insecure
>   certs:
>     - name: insecure
>       skip_tls: true
> ```
>
> The `plaintext_default` system-error reason was retired (that state is now unreachable);
> alert on `edgion_system_error_state{reason="skip_tls"}` instead.
> Generate certificates with `edgion-cli gen-certs --output file --output-dir <dir>`.

### [conf_sync_security.rotation] — conf_sync Auto-CA Cert Rotation

Optional; **consumed by the Controller only** — the Gateway parses-and-ignores this
block (unknown-to-the-gateway config, WARN only, never a startup failure). **Applies
only when `conf_sync_security.auto_ca: true`** (Kubernetes mode). In file mode
(external cert supply, `auto_ca` omitted/false) this block has no effect on the leaf/CA
lifecycle — but the **hot-reload behavior still applies** in file mode (see the Gateway
config doc's client-cert hot-reload section for the consumer side; the Controller has
its own analogous server-side hot reload). See
`docs/superpowers/specs/2026-07-12-conf-sync-cert-rotation-design.md` for the full design.

> **`auto_ca` is safe under both `ha_mode` values** (`leader-only` and `all-serve`).
> Secret WRITES (CA renewal + the shared gateway client leaf) stay leader-only, but each
> replica independently keeps **its own** conf_sync server identity — the served leaf AND
> the client-CA verifier that authenticates gateways — in sync with the current root, via a
> 60s read-only reconciler that runs on every replica (`services/conf_sync_ca/identity_sync.rs`,
> mirroring istiod's `watchRootCertAndGenKeyCert`). This is what makes an emergency CA
> re-key actually revoke: without it, a non-leader under `all-serve` would keep accepting
> the old (compromised) gateway client cert — and keep streaming resolved Secrets to it —
> until that pod happened to restart. The reconciler is read-only on Secrets, so it needs no
> coordination and cannot split-brain the root.

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `policy` | `"lifetime" \| "scheduled" \| "manual"` | `"lifetime"` | Rotation trigger strategy (see below) |
| `lifetime_ratio` | `f64` | `0.5` | `lifetime` policy only: rotate once elapsed leaf age reaches this fraction of `leaf_ttl`. Must be in `(0.0, 1.0)` exclusive |
| `schedule` | `String?` | `None` | 5-field crontab (e.g. `"0 3 * * *"`). **Required** iff `policy: scheduled`; ignored otherwise |
| `schedule_timezone` | `String` | `"UTC"` | IANA timezone the `schedule` cron is evaluated in |
| `rotate_if_remaining_below` | `CertDuration` | `"336h"` (14d) | `scheduled` policy only: guard threshold — rotate at a firing iff the leaf's remaining validity *at that firing time* is `<=` this value |
| `emergency_rotate_below` | `CertDuration` | `"24h"` | Always-on safety-net threshold, independent of `policy` (see "Emergency safety net" below) |
| `leaf_ttl` | `CertDuration` | `"365d"` | Validity period minted for new client/server leaf certs |
| `ca_ttl` | `CertDuration` | `"3650d"` | Validity period minted for a new root CA (renewal keeps the same key, only extends validity) |

**The three policies:**

- **`lifetime`** (default): rotate the client leaf once its elapsed age reaches
  `lifetime_ratio × leaf_ttl` (e.g. at the default 0.5/365d, rotate at ~182.5 days).
  Evaluated every reconciler tick against the actual leaf window — no cron involved.
- **`scheduled`**: rotate only inside a low-traffic cron window. The guard is evaluated
  **at the firing time**, not at "now": a firing is due iff
  `leaf_not_after − firing_time <= rotate_if_remaining_below`. This is deliberate —
  evaluating the guard against `now` instead of the firing collapses the policy into a
  plain remaining-threshold trigger and defeats the point of a scheduled window (spec
  §4.2). A firing that has not yet happened, or one the leaf has already been rotated
  for (via `BACKDATE_SLACK` bookkeeping), never re-triggers.
- **`manual`**: the reconciler never auto-rotates on lifetime/schedule grounds; rotation
  only happens via the manual API below (the always-on emergency net below still
  applies regardless of policy).

**What `policy` does NOT govern: each replica's own server identity.** The three policies
above control when the **shared gateway client leaf Secret** (`edgion-conf-sync-certs`)
changes — a change that disrupts *every* gateway in the fleet and therefore belongs in a
maintenance window. They do **not** apply to the per-replica reconciler
(`identity_sync.rs`), which re-signs **this replica's own in-memory server leaf** — and
rebuilds the client-CA verifier that authenticates gateways — whenever it drifts from the
current root, or at plain half-life (`lifetime_ratio`), **regardless of `policy`**. That
asymmetry is deliberate: re-signing a private, in-memory server leaf disrupts nothing
(established TLS sessions are untouched — certificates are consumed only at handshake —
and new handshakes simply pick up the new cert), so there is nothing to defer. Deferring
it under `policy: manual` would instead recreate the very bug the reconciler exists to fix
(a replica serving behind a stale trust root after an emergency re-key). Each re-sign logs
`conf_sync_server_identity_resigned`.

**Always-on emergency safety net.** Independent of `policy`, if a leaf's remaining
validity drops to `<= emergency_rotate_below` (default `24h`), the reconciler rotates
immediately and logs `WARN event="cert_emergency_rotation"`. Unlike the policy conjunct,
the net checks **both** the client leaf and the (in-memory, re-minted-per-restart)
server leaf window — a long-running controller under `policy: manual` can still expire
its own server leaf. **Evaluation order**: the configured policy conjunct is checked
first; the emergency net only fires when the policy itself says not-due. This means a
healthy on-time `scheduled`/`lifetime` rotation is always labeled `Due`, never
`EmergencyDue` — `EmergencyDue` is reserved for the case where the policy trigger alone
would have missed the window (mis-sized guard, extended leader outage, forgotten manual
rotation). Rotation coverage is unaffected either way; only the log/reason label
differs.

**Duration units (`CertDuration`).** Form `<integer><unit>`, single term only:
`h` (hour), `d` (=24h), `w` (=7d), `mo` (**fixed 30d approximation**, not calendar
month), `y` (**fixed 365d approximation**, not calendar year). Zero (`"0h"`) and
integer overflow are rejected at parse time; a bare unitless number (e.g. `36`) is also
rejected — the unit suffix is mandatory.

**Validation invariants** (checked at startup only — `conf_sync_security.rotation` is
not re-validated on config reload; violation aborts startup):

- `leaf_ttl >= 1h`, `ca_ttl >= 1d`, `leaf_ttl < ca_ttl`.
- `emergency_rotate_below < leaf_ttl` (always).
- `policy: scheduled` additionally requires: `schedule` present;
  `emergency_rotate_below < rotate_if_remaining_below`; and
  `rotate_if_remaining_below <= 0.5 × leaf_ttl` (the "churn hole" guard — a wider window
  would let the guard span more than half the leaf's life and risk rotating on
  consecutive firings).
- `policy: lifetime` additionally requires: `lifetime_ratio` in `(0.0, 1.0)` exclusive;
  and `emergency_rotate_below < (1 − lifetime_ratio) × leaf_ttl` (the emergency net must
  not always preempt the lifetime trigger).

**366-day cron feasibility check** (`policy: scheduled` only, Controller startup):
the configured `schedule`/`schedule_timezone` is simulated over the next 366 days.

- **Never fires within the horizon** (e.g. `0 3 30 2 *` — Feb 30 never exists) →
  startup **reject**: almost certainly a typo.
- **Fires, but `rotate_if_remaining_below` is less than 2× the max gap between
  consecutive firings** → startup **WARN** + `mark_system_error` with reason
  `cert_rotation_config_suspect` (metric `edgion_system_error_state{reason="cert_rotation_config_suspect"}`):
  a guard this tight relative to the firing cadence risks a due window being missed
  entirely between two firings. Not rejected — the operator may have deliberately chosen
  a tight guard for a fast-firing schedule.

**Manual rotation API**: `POST /api/v1/services/conf-sync-ca/rotate`

Request body `{"scope": "leaf"|"ca"}` (default `"leaf"`).

| Status | Meaning |
|--------|---------|
| `202 Accepted` | Enqueued on the elected leader replica |
| `409 Conflict` | This replica is not the leader; retry against the `edgion-controller-leader` Service |
| `503 Service Unavailable` | Leader, but the rotation worker is not yet running (mid-init) or its queue is full |
| `400 Bad Request` | `auto_ca` is disabled — there is no managed rotation to trigger; replace the cert files on disk and rely on hot reload instead |

**Alerting**: the rotation pipeline is covered by three signal layers with a deliberate
division of labor — do not page on everything just because a signal exists:

- **PAGE on** (act now):
  - `edgion_conf_sync_cert_expiry_timestamp_seconds - time() < 7 * 86400` — the
    certificate actually in service is about to expire. This is the external backstop
    (`skills/03-coding/observability/01-metrics.md`'s arbitrated-exception gauge): it
    watches the live cert independent of whatever is supposed to be rotating it, so it
    still fires even if the rotation pipeline itself is wedged.
  - `edgion_system_error_state{reason="cert_rotation_failed"} == 1` sustained 5m — the
    **leader's** rotation pipeline (the Secret-writing one) is actively failing (not yet
    expired, but the mechanism meant to prevent that is broken). Set on a failed tick,
    cleared on the next successful one.
  - `edgion_system_error_state{reason="cert_identity_sync_failed"} == 1` sustained 5m —
    **this replica cannot resync its own conf_sync serving identity** against the current
    root (the per-replica reconciler's tick is failing). It is deliberately a *separate*
    reason from `cert_rotation_failed`: the two loops both run on the leader under the
    same `conf_sync_server` component, and if they shared a reason the read-only identity
    loop — which succeeds on almost every tick — would clear the rotation loop's mark
    every 60s and silence the page above. Page on it because a replica that cannot resync
    its identity is a replica that **may still trust a revoked root**: under
    `ha_mode: all-serve` it keeps serving resolved Secrets and may keep accepting a
    gateway client leaf that an emergency re-key was supposed to revoke. Set on a failed
    tick, cleared on the next successful one.
  - `edgion_system_error_state{component="conf_sync_client",reason="confsync_disconnected"} == 1`
    sustained 10m — **that gateway's conf_sync link to the controller is down and has not come
    back**. Scraped from the *gateway* pods, not the controller: each gateway raises this on its
    own registry, so the pod that is broken is the one that tells you. It is **not** a data-plane
    outage — the gateway keeps serving traffic from its last-known config, which is deliberate —
    but its config is frozen: every subsequent route/backend/cert change silently does not reach
    it, so a deploy "succeeds" while that pod keeps serving the old world. Nothing else catches
    this; `/ready` latches true at boot by design and stays green throughout.
    Use **10m**, not the 5m above: a controller rollout legitimately flips this to 1 for a few
    seconds, and the gateway's own reconnect backoff caps at 30s. Scope: this tracks the
    `WatchServerMeta` stream, so an application-level failure confined to the per-kind resource
    watches (unparseable list response, persistent server-id mismatch) will *not* raise it — see
    the reason's doc comment in `edgion-common/src/observe/system_metrics.rs`.
- **TICKET on** (review, not urgent):
  - `edgion_system_error_state{reason="cert_rotation_config_suspect"} == 1` — the rotation
    config combination needs a look (suspect firing-gap-vs-guard sizing, or a healthy
    steady state that lands inside the expiry-watch WARN band — see the Validation
    invariants above; the default scheduled guard of 336h is one such case and is
    expected, not broken).
  - `edgion_system_error_state{reason="cert_watch_failed"} == 1` — the filesystem watcher
    backing cert hot reload could not be established (watcher construction failed, or a cert
    parent directory could not be watched), on either the controller (`conf_sync_server`) or
    the gateway (`conf_sync_client`). **This is not an outage**: hot reload has degraded to
    the 5-minute periodic re-stat backstop, so a rotated cert is still picked up — just in
    minutes rather than seconds. The gauge **latches** (the watch is never retried), so it
    stays at 1 until the process restarts; it is a fix-at-leisure ticket, not a page.
  - Expiry-watch WARN-band transition logs (`component="cert_expiry_watch"`, 30-day band)
    — worth a look, not worth waking anyone up; the ERROR/7-day band is what pages, above.
- **LOG-ONLY** (triage context; do not page):
  - `conf_sync_server_identity_resigned` (INFO, `component="conf_sync_ca"`) — a replica
    re-signed **its own** conf_sync server leaf (and rebuilt its client-CA verifier)
    against the current root. Security-relevant and worth grepping for during an
    incident: after an emergency CA re-key, **every** controller pod must log one of
    these (or, on the leader, a `conf_sync_cert_rotated`) — that is what says the
    revocation actually reached that pod, and it is what
    `rekey_conf_sync_ca.sh` asserts per pod. Steady-state occurrences (half-life
    re-signs, a same-key root renewal) are routine, so this is not a page. Carries a
    `reason` field (`does not chain` / `trust anchor is not the current root cert` /
    `emergency window` / `lifetime ratio`) plus the old/new serials.
  - `cert_emergency_rotation` (WARN) — the safety net already acted and rotated the leaf;
    useful to know the primary policy trigger was late, but nothing is broken now.
  - Reload-reject WARNs (a bad candidate cert/key file was rejected and the previous
    identity stayed in service) — the `reason="cert_reload_invalid"` system-error gauge
    (`edgion_system_error_state{reason="cert_reload_invalid"}`) already covers paging if
    these become sustained; a single rejection is triage context, not an incident.

**Emergency: CA root re-key (private-key compromise)**

**When**: a leaf or root private key has leaked. Routine same-key CA renewal
(the CA-renewal due-check above) and the manual-rotate API's `scope=ca` do
**not** revoke anything — the root's public key is unchanged, so old leaves
still verify against it. A full root **re-key** (new key pair, not just a new
window) is the only true revocation.

- **Kubernetes auto-CA procedure** — use the script. It performs the steps below and then
  asserts the post-conditions, proving **both halves** of a revocation — that the NEW root
  works everywhere, *and* that the OLD key no longer does — on **every controller and
  every gateway**. Any failed assertion exits non-zero and prints
  `RE-KEY INCOMPLETE`; "cannot check" never reads as "passed":

  | Proof | Assertion |
  |-------|-----------|
  | New root is really new | the root's **SPKI changed** (a new key pair, not a same-key renewal, which revokes nothing) |
  | Leaf Secret | `edgion-conf-sync-certs`'s `client.crt` chains to the new root |
  | Every **gateway** | its **mounted** `client.crt`, read off the pod, chains to the new root (restart-proof) |
  | Every **controller** (positive) | port-forward + `openssl s_client` → the leaf it **actually serves** chains to the new root. This is the only way to see a non-leader's in-memory trust root; under `ha_mode: all-serve` every replica serves resolved Secrets behind its own copy of it |
  | Every **controller** (negative, new handshakes) | the **pre-re-key client cert+key** (captured before the delete) are **REJECTED** in an mTLS handshake. Everything else only shows a new key started working, not that the leaked one stopped. A successful old-key handshake is a hard failure: the compromise is NOT contained |
  | **Live session** (negative) | an mTLS session opened with the OLD client identity **before** the delete and held open **across** the re-key must be **TORN DOWN**. This is the attacker's actual position — they never have to re-handshake — and no other check in the suite can see it. A surviving session is a hard failure; a session whose *port-forward* died is **inconclusive**, not a pass |
  | Every **controller** (logs) | a NEW `conf_sync_cert_rotated` (leader) **or** `conf_sync_server_identity_resigned` (any replica), asserted **per pod** — a fleet-wide sum would let the leader's single event vouch for replicas that never moved. Unreadable logs are reported as UNKNOWN, never as a pass |

  A run whose old client identity could not be captured (leaf Secret absent/malformed), or
  that could not open the pre-delete session, **exits non-zero**: it can only show that a
  new root started working, never that the leaked one stopped.

  ```bash
  cicd/deploy/kubernetes/scripts/rekey_conf_sync_ca.sh            # dry run: plan + current root
  cicd/deploy/kubernetes/scripts/rekey_conf_sync_ca.sh --yes      # re-key, then verify
  cicd/deploy/kubernetes/scripts/rekey_conf_sync_ca.sh --verify   # re-check post-conditions later
  ```

  What it does under the hood:
  `kubectl -n <ns> delete secret edgion-conf-sync-ca edgion-conf-sync-certs`.
  The elected leader's next tick (≤60s) sees the leaf Secret missing, re-bootstraps
  a fresh root + both leaves, and hot-swaps its own in-process server identity.
  Every **other** controller replica re-signs its own serving identity — and the
  client-CA verifier that authenticates gateways — on its own 60s tick
  (`identity_sync.rs`); that is what makes the re-key actually revoke under
  `ha_mode: all-serve`, and it is what the script's per-controller proofs assert.
  Gateways self-heal via the existing Secret-mount hot-reload path (kubelet sync
  + the 2s debounce, ≤ ~2min total to fully propagate) — **no process restarts
  required, on either side**.

  **Rotation does not disturb live sessions; a re-key does — and that is the point.**
  A certificate is consumed once, at handshake, so a routine leaf rotation (same trust
  root) deliberately leaves established conf_sync streams running: the peer is still
  authorized, and zero disruption is the whole reason for the hot-swap design. A
  **trust-root change** (this re-key, or an out-of-band BYO-root swap) is different: the
  peers on those sessions were authenticated by a root that is now revoked, and on an
  established HTTP/2 connection they never re-handshake — they would keep issuing
  `List`/`Watch` and keep receiving resolved Secrets (TLS private keys, backend
  credentials) forever. So each controller scopes every accepted connection to the trust
  root that authenticated it and **cancels those connections when the root changes**
  (`tls_identity.rs` trust generation → `grpc_server.rs` `TrustScopedStream`). Gateways
  reconnect and relist; the data plane keeps serving throughout. There is deliberately no
  blanket connection-age cap: it would recycle every healthy connection (a full relist
  each time, since all gateways share one client-leaf Secret) for no benefit outside a
  re-key. The re-key script proves the teardown on every re-key instead.

  A leader failover happening during or immediately
  after the re-key also self-heals within one tick: the reconciler's
  chain-consistency check (`edgion-controller/src/services/conf_sync_ca/rotation.rs`
  `out_of_sync_reason`) detects that the new leader's in-memory server identity
  (minted at its own startup, against the now-deleted root) no longer chains to
  the current CA, and rotates the leaves immediately regardless of policy.
  Expected transient: TLS handshake failures during the propagation window — the
  controller logs rate-limited `conf_sync TLS handshake failed` WARNs (with the peer),
  and the affected gateways raise
  `edgion_system_error_state{reason="confsync_disconnected"}`. Both subside on their
  own once every peer has picked up the new root.
- **Verification checklist** (the script asserts all of the mechanical ones — this is
  what to eyeball if you are doing it by hand):
  - A new `edgion-conf-sync-ca` Secret exists with a different `ca.crt` (compare
    against a saved copy, or just check the Secret's `creationTimestamp`).
  - The leader logs `conf_sync_cert_rotated` with reason `"leaf secret missing"`, and
    **every other controller pod** logs `conf_sync_server_identity_resigned`.
  - Every controller pod serves a leaf chaining to the new root **and rejects the old
    client key** (see the script's per-controller proofs; a pod that still accepts the
    old key has not revoked anything).
  - Gateway log shows `conf_sync_cert_reloaded` followed by a successful reconnect.
  - No gateway is stuck at `edgion_system_error_state{reason="confsync_disconnected"} == 1`
    (one that is has not reconnected to the controller and is serving stale config —
    grep that pod's controller-side `conf_sync TLS handshake failed` WARN for the
    reason), and `edgion_system_error_state{reason="cert_identity_sync_failed"}` is `0`
    on every replica (a replica stuck at 1 may still be trusting the compromised root).
  - The expiry gauges (`edgion_conf_sync_cert_expiry_timestamp_seconds`) reflect
    the new certs' timestamps.
- **File / solo mode**: there is no auto-CA to delete a Secret for — regenerate the
  CA and both leaves externally (e.g. `edgion-cli gen-certs`), then replace the
  files on **both** sides. Order doesn't matter much, but do it promptly: the
  mismatch window between replacing one side and the other is exactly the
  handshake-failure window. Hot reload picks up each side's replacement within
  ~2s once written.
- **Do NOT use the manual-rotate API for compromise response.** It goes through
  the (possibly compromised) control plane, and for `scope=ca` it performs the
  same-key renewal described above — a new window, not a new key, so it revokes
  nothing.

### [conf_center] — FileSystem

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `type` | `String` | — | `"file_system"` |
| `conf_dir` | `PathBuf` | `conf` | Resource configuration directory |
| `poll_interval_ms` | `u64?` | Watcher-calculated | Poll interval override; minimum 100 ms |

Important practical constraints:
- `conf_dir` is currently resolved by `FileSystemStorage::new()` directly against the process cwd
- That is, it does not automatically follow `work_dir` like many log paths do
- Prefer absolute paths when you want to avoid ambiguity

CLI special cases:
- `--conf-dir` directly overrides this `conf_dir`
- It also switches `conf_center` to the FileSystem variant

### [conf_center] — Kubernetes

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `type` | `String` | — | `"kubernetes"` |
| `gateway_class` | `String` | **Required** | Name of the GatewayClass to match |
| `controller_name` | `String` | — | controllerName reported in Gateway API status |
| `watch_namespaces` | `Vec<String>` | `[]` (all) | List of namespaces to watch |
| `label_selector` | `String` | `""` | Resource label filter |
| `gateway_address` | `String?` | — | Address reported in Gateway status |
| `ha_mode` | `String` | — | `"leader-only"` or `"all-serve"` |
| `no_watch_kinds` | `Vec<String>` | `[]` | Resource kinds (PascalCase) not to probe/watch, e.g., `["TCPRoute", "UDPRoute", "BackendTLSPolicy"]`. Skips CRD precheck at startup, does not spawn a watcher, and is excluded from processor readiness |
| `cache_ready_deadline_secs` | `u64` | `90` | Deadline (seconds) for a started watcher set to reach `CachesReady` (covers the internal 30 s readiness-poll budget plus the post-ready tail: WatchObj registration, revalidations, leader-job start). **Must be `> 30`** — a startup validation error otherwise. **Restart-bound, not hot-reloadable**: `POST /api/v1/reload` re-arms the deadline every iteration but always with the boot-time value; changing it requires a process restart. Sizing rule of thumb: raise it when a single resource kind's count approaches the per-kind workqueue capacity (5000) — a too-small value causes a permanent restart loop, not a one-off failure. Worst-case pre-ready wait before a restart fires is ≈2× the configured value (the deadline also wraps `start_event_watchers` itself via a timeout, then re-arms fresh for the tail). See [Supervision & recovery](../../01-architecture/01-controller/03-config-center/02-kubernetes/00-lifecycle.md#supervision--recovery) for the full mechanism. |

### [conf_center.leader_election]

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `lease_name` | `String` | `edgion-controller-leader` | K8s Lease name |
| `lease_namespace` | `String` | `POD_NAMESPACE`, else `default` | K8s Lease namespace |
| `lease_duration_secs` | `u64` | `15` | Lease duration in seconds (validity on the apiserver side) |
| `renew_deadline_secs` | `u64` | `10` | Renewal deadline window: within this window after the most recent successful renewal, the Leader retains leadership and retries even when transient errors (5xx / network) occur, avoiding spurious leader flips caused by apiserver flapping. Constraint: `retry_period_secs < renew_deadline_secs < lease_duration_secs`. |
| `retry_period_secs` | `u64` | `2` | Uniform sleep interval between each acquire/renew attempt (corresponds to client-go `RetryPeriod`). Roughly `renew_deadline_secs / retry_period_secs` retries occur within a single `renew_deadline_secs` window. |

### [conf_center] — Etcd

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `type` | `String` | — | `"etcd"` |
| `endpoints` | `Vec<String>` | **Required** | etcd cluster endpoints |
| `key_prefix` | `String` | `/edgion` | Prefix for configuration and status keys |
| `auth` | `{username,password}?` | — | Optional basic credentials; secret material |
| `tls` | object? | — | Optional CA and client certificate/key file paths |
| `connect_timeout_ms` | `u64?` | Provider default | Dial timeout |
| `request_timeout_ms` | `u64?` | Provider default | Per-request timeout |
| `keep_alive_time_secs` | `u64?` | Provider default | HTTP/2 keepalive interval |
| `keep_alive_timeout_secs` | `u64?` | Provider default | HTTP/2 keepalive timeout |

### [conf_center.metadata_filter]

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `blocked_annotations` | `Vec<String>` | Common large kubectl/helm annotations | Annotations removed before entering memory |
| `remove_managed_fields` | `bool` | `true` | Reduce object size |

## `admin_token` — Admin API Authentication

The Admin API uses **token-only** authentication (Istio-style pre-shared bearer token). There is no OIDC admin, no username/password, and no httpOnly cookie. The operator supplies a single admin token that grants allow-all (wildcard `*`/`*` RBAC policy) access to the Admin API.

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `admin_token` | `Option<String>` | `None` | Raw bearer token. A blank/whitespace value is treated as **not configured**. The `EDGION_ADMIN_TOKEN` env var takes precedence over this field. Maps to an allow-all wildcard policy; survives reloads. |

The token is resolved from two sources (highest precedence first):

1. **Env var** `EDGION_ADMIN_TOKEN`, injected via `valueFrom.secretKeyRef` from the `edgion-admin-credentials` Secret (key `token`; recommended for Kubernetes).
2. **`admin_token`** config field (a Secret-mounted config file).

```yaml
# Prefer the env var over an in-file value:
admin_token: "edg_…"
```

When **no** token is configured (neither `EDGION_ADMIN_TOKEN`/`admin_token` nor any `cli_tokens`), the server still boots, but all Admin API business routes return `503` fail-close with a startup WARN. `/health` / `/ready` / `/metrics` / `/api/v1/auth/status` remain accessible. Scoped, role-limited tokens are configured separately under [`[cli_tokens]`](#cli_tokens--static-cli-tokens-admin-api-access).

See `skills/02-features/02-config/03-auth-bootstrap.md` for the full matrix and migration guide.

## `[center]` — Federation Client

Connects this Controller to a Federation Center. Omit this section entirely if you are not using federation.

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `address` | `String` | **Required** | gRPC address of the Center server (e.g. `https://center.example.com:12251`) |
| `name` | `String` | **Required** | Logical name of this Controller, reported to Center |
| `cluster` | `String` | **Required** | Cluster name, used for routing and aggregation |
| `enabled` | `bool` | `true` | Hot-reloadable kill-switch. `false` stops the federation client without a restart. Reloaded via `POST /api/v1/reload`. |
| `env` | `Vec<String>` | `[]` | Environment tags reported to Center (e.g. `["production"]`). |
| `tag` | `Vec<String>` | `[]` | Arbitrary extension tags reported to Center. |
| `security` | `ConfSyncSecurityConfig` | — | mTLS certificates for the gRPC channel (same format as `conf_sync_security`). Required -- federation fails closed without it. |
| `rbac` | `FedRbacConfig?` | `null` | RBAC for Center operations. **Absent => built-in default policy applies (read `get`/`list`/`watch`/`list-keys` on all ResourceKinds except `Secret`; write `create`/`update`/`delete` on `EdgionConfigData`; `list`/`failover` on synthetic `RegionRoute`; `server-info`/`reload` on synthetic `NonResource`). Present => the explicit rules fully override the default.** |

### `[center.rbac]` -- Federation RBAC

**When `rbac` is absent** (but `[center]` is present), a built-in default policy applies:

| What | Allowed |
|------|---------|
| Read (`get`, `list`, `watch`, `list-keys`) | All resource kinds **except** `Secret` (EdgionTls IS readable; only the Secret kind is excluded for credential safety) |
| Write (`create`, `update`, `delete`) | `EdgionConfigData` only (drives IP-restriction sync + region-route sync; grants write to ALL plugin metadata) |
| `list`, `failover` | `RegionRoute` (synthetic label): `list` covers the effective read endpoint (`GET /api/v1/region-routes/effective`); `failover` covers the POST failover endpoint. No route classifies as `(get, RegionRoute)`. |
| `server-info`, `reload` | `NonResource` (synthetic label): read-only server identity/readiness checks, plus the Center reload button arriving as a proxied `POST /api/v1/reload` |
| Everything else (`diagnostics`, `wipe-all`, `debug`, etc.) | Denied |

**When `rbac` is present**, the explicit rules fully override the built-in default. This is how operators grant more (e.g., Secret read or reload) or less than the default.

> **Security:** There is no server-side Secret backstop on the federation path. A wildcard read (`kinds: ["*"]` with `get`, `list`, or `watch`) lets Center read Secret / EdgionTls private-key material via the blind proxy. Always enumerate concrete kinds for any UI surface. The built-in default avoids this by excluding `Secret` explicitly.

**Example: explicit minimal allowlist (automated operations only):**

```yaml
center:
  address: "https://center.example.com:12251"
  name: "ctrl-1"
  cluster: "prod"
  enabled: true
  security:
    active: primary
    certs:
      - name: primary
        cert: certs/grpc/primary/client.crt
        key: certs/grpc/primary/client.key
        ca: certs/grpc/primary/ca.crt
  rbac:
    allow:
      - verbs: [get, list, watch, create, update, delete]
        kinds: [EdgionConfigData]
      - verbs: [failover]
        kinds: [RegionRoute]
      - verbs: [reload]
        kinds: ["*"]
```

| Rule | Covers |
|------|--------|
| `get/list/watch/create/update/delete` on `EdgionConfigData` | IP-restriction plugin sync + region-route sync; `list` also serves Center blind-proxy UI list pages |
| `failover` on `RegionRoute` | Regional failover endpoint |
| `reload` on `*` | Config reload (non-resource, wildcard kind is safe for write verbs) |

> **Note on `kinds: ["*"]` with `reload`:** using `*` is safe here because `reload` is a write-only action with no read surface. The wildcard-read prohibition applies to `get`, `list`, and `watch` only.

> **Cross-reference:** this change also fixes review finding `controller-01` -- the federation path now routes through the Admin API authorization layer instead of a bare unauthenticated router. See `skills/04-review/federation/regression-guards/fed-proxy-mtls-fail-close.md` for the prior mTLS fix and the new RBAC fix status.

> **Full operator guide:** see `docs/en/ops-guide/federation-rbac.md` for authorization examples, security notes, and the kill-switch reference.

`FedAllowRule.verbs` accepts: `get`, `list`, `list-keys`, `watch`, `create`, `update`, `delete`, `failover`, `acme-trigger`, `conf-sync-rotate`, `reload`, `server-info`, `diagnostics`, `wipe-all`, `debug`.

> **Kind casing:** `kinds` MUST use the canonical PascalCase resource Kind name (e.g. `EdgionConfigData`, `HTTPRoute`, `RegionRoute`). Lowercase or alias spellings (e.g. `edgionconfigdata`) are matched case-sensitively against the normalized request kind and will silently never match. Verbs are case-insensitive.

## `[cli_tokens]` — Static CLI Tokens (Admin API Access)

Static Bearer tokens that grant role-scoped access to the Controller Admin API. Each entry binds a token to a fine-grained RBAC policy using the same `(verb, kind)` model as `[center].rbac`. Tokens are never stored in plaintext — only their SHA-256 hex digest is persisted. Hot-reloadable.

**Source:** `edgion-controller/src/config/mod.rs` (`CliTokenConfig`).

### Field Reference

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `name` | `String` | **Required** | Human-readable label used in audit/log fields. Must match `[A-Za-z0-9_.-]+`. |
| `token_sha256` | `String` | **Required** | SHA-256 hex digest (64 lowercase hex chars) of the raw bearer token. Never store the token itself. Mint via `edgion-cli gen-token`. |
| `rbac` | `FedRbacConfig` | deny-all | RBAC allow-rules for this token. Absent or empty ⇒ deny-all. Same `verbs × kinds` schema as `[center].rbac`. |

### Config Example

```yaml
cli_tokens:
  # Read-only token — safe for monitoring/CI pipelines that only inspect routes
  - name: monitor-bot
    # Mint the token with: edgion-cli gen-token
    # Paste the SHA-256 line here; distribute the edg_… token to the consumer.
    token_sha256: "0000000000000000000000000000000000000000000000000000000000000000"
    rbac:
      allow:
        # Enumerate concrete kinds; NEVER use kinds: ["*"] with read verbs
        # (wildcard read exposes Secret and EdgionTls private-key material).
        - verbs: [get, list]
          kinds: [HTTPRoute, GRPCRoute, TCPRoute, Service, EdgionGatewayConfig, EdgionTls]

  # Scoped read-write token — manages plugin metadata and can trigger reload
  - name: plugin-manager
    token_sha256: "1111111111111111111111111111111111111111111111111111111111111111"
    rbac:
      allow:
        - verbs: [get, list, watch, create, update, delete]
          kinds: [EdgionConfigData]
        # reload is a write-only verb; kinds: ["*"] is safe here (no read surface)
        - verbs: [reload]
          kinds: ["*"]
```

> **Kind casing:** `kinds` MUST use the canonical PascalCase resource Kind name (e.g. `EdgionConfigData`, `HTTPRoute`). Lowercase spellings silently never match.
>
> **Full operator guide:** see `docs/en/ops-guide/federation-rbac.md#cli-token-rbac` for the gen-token workflow, security notes, and rotation instructions.

## `path_normalization` — Path Normalization (Gateway-side configuration)

Path normalization configuration is read on the Gateway side (`edgion-resources/src/path_normalization/config.rs`); the Controller config does not directly own this section, but skills documentation records it here for unified reference.

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `reject_escaped_slashes` | `bool` | `false` | Reject requests containing `%2F` / `%5C` (high-security scenarios) |
| `reject_invalid_percent_encoding` | `bool` | `false` | Reject requests containing invalid percent-encoding (overlong UTF-8, etc.) |
| `forward_normalized_path` | `bool` | `true` | (v4) Forward the normalized path to the upstream by default. When set to false, all routes default to forwarding raw (compatible with v3.1 behavior, soft-fallback channel). For per-route override see annotation `edgion.io/forward-raw-path`. |
| `preserve_path_parameters` | `bool` | `false` | (v4) Preserve path-parameter literals (`;param=...`). Default false (stripped, consistent with v3.1, preserving the §1.4 path-parameter bypass guarantee). When set to true, `..;/` -style bypasses move out of the default guarantee scope (see spec §14.4 for details). |

## Most Common Controller Configuration Changes in This Project

- Switch the configuration directory for local testing: change `conf_center.type = "file_system"` + `conf_dir`
- Limit a K8s deployment to a single GatewayClass: change `gateway_class`
- Control cache caps for different kinds: change `conf_sync.capacity_overrides`
- Tune observability noise: change `logging.log_level`
