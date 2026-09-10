# GeoIP Restriction

> **Gateway-local feature.** GeoIP database configuration lives in the gateway's local startup config
> (not a CRD) because the database, its memory map, and the background updater are one
> process-wide resource, not something that can vary per named `EdgionGatewayConfig` instance.
> Changing any `geoip` field requires a pod restart. Enforcement itself is a normal CRD-managed
> plugin, `GeoIpLocation`, and is hot-reloadable like any other plugin.

GeoIP lets you restrict traffic by the geographic origin of the client and enriches the access
log with the resolved country/continent/city. Both are provided by **one plugin, `GeoIpLocation`**,
registered with the identical config shape into both layers:

- **HTTP** — `EdgionPlugin::GeoIpLocation` in `EdgionPlugins.spec.requestPlugins`.
- **L4 stream lifecycles** — `EdgionStreamPlugin::GeoIpLocation` in
  `EdgionStreamPlugins.spec.plugins`. The same chain runs at listener admission (`Stage1`),
  on TCPRoute and UDP transport-route admission (`Tcp`). TLS traffic uses the Gateway
  listener attachment (`Stage1`); there is no `tlsRoutePlugins` registration for GeoIP.

**Not supported, by design**: no bundled baseline database (redistribution terms), no
coordinate/radius geofencing (MMDB coordinates are city-centroid values with a coarse
`accuracy_radius`), no ASN lookup, and no readiness-probe gating on database availability (see
[No readiness gating](#no-readiness-gating-the-operator-owns-the-cold-start-window)).

## Configuration

`geoip` is a block inside the gateway's local `GatewayConfig` — **it must be nested under the
top-level `gateway:` key**, not written at the document root:

```yaml
# edgion-gateway.yaml
gateway:
  geoip:
    db_path: /var/lib/edgion/geoip/db.mmdb
    # Empty/absent => load the local file only; the updater is never spawned.
    update_url: https://mirror.internal/geo.mmdb
    # Name of an env var whose value is sent verbatim as the Authorization header.
    # Credentials never go in update_url or a CLI flag.
    update_auth_header_env: GEOIP_UPDATE_AUTH
    update_interval_sec: 86400
    retry_interval_sec: 600
    connect_timeout_sec: 10
    download_deadline_sec: 600
    stall_timeout_sec: 60
    max_db_bytes: 268435456        # 256 MiB
```

### 🚨 The single most common way to get this wrong

`GatewayConfig` is embedded in the root config struct with `#[command(flatten)]` for the CLI,
but **not** `#[serde(flatten)]` for YAML. That means a `geoip:` block written at the **document
root** — one level too high — deserializes cleanly, is silently dropped by the outer struct, and
leaves `GeoIpConfig` at its empty defaults (no `db_path`, no `update_url`). There is **no error,
no warning, no startup log line**. This is not the kind of mistake `deny_unknown_fields` catches,
because `deny_unknown_fields` is on `GeoIpConfig` itself — the unrecognized key is dropped one
level above it, on the field that owns `geoip`, before `GeoIpConfig`'s own deserializer ever runs.

```yaml
# WRONG — silently produces an empty GeoIpConfig; no error at startup
geoip:
  db_path: /var/lib/edgion/geoip/db.mmdb

# RIGHT — nested under `gateway:`
gateway:
  geoip:
    db_path: /var/lib/edgion/geoip/db.mmdb
```

This exact mistake cost real debugging time during implementation — it was first mistaken for a
conf_sync propagation race, because everything else about the deployment looked correct and the
gateway started up cleanly. If geo rules appear to have no effect and `edgion_geoip_db_loaded`
stays `0` while your YAML clearly sets a `db_path`, check indentation first.

### Field reference

All `geoip` keys are **snake_case** (matching the rest of `GatewayConfig`, unlike the CRD-facing
resources which are camelCase). The struct carries `#[serde(deny_unknown_fields)]`, so a
camelCase typo *inside* the `geoip` block (`dbPath` instead of `db_path`) is a hard config-load
error, not a silent default — this is the one layer where the guard actually catches a typo. The
trap above is one level higher, where no `deny_unknown_fields` exists to catch it.

| Field | Type | Default | Description |
|---|---|---|---|
| `db_path` | path | none | Local MMDB path. May not exist at first start — see [Database file lifecycle](#database-file-lifecycle). There is no separate master switch: an unset `db_path` means no database, and geo lookups report `Unavailable`. |
| `update_url` | string | none | Update source. Empty/absent means load the local file only and never spawn the updater. Redacted in every log line. |
| `update_auth_header_env` | string | none | Name of an environment variable whose value is validated as the HTTP `Authorization` header. Invalid values are rejected without being echoed. In Kubernetes, inject it from a Secret via `secretKeyRef`. |
| `update_interval_sec` | u64 | `86400` | Steady-state interval between update cycles (after the first successful load). Must be greater than zero when `update_url` is set. |
| `retry_interval_sec` | u64 | `600` | Steady-state retry backoff after a failed cycle, **once a database has already loaded successfully once**. Not used for the very first fetch — see [cold-start backoff](#multi-replica-update-behavior-jitter-and-the-meta-validator). Must be greater than zero when `update_url` is set. |
| `connect_timeout_sec` | u64 | `10` | TCP connect timeout for the update request. Must be greater than zero when `update_url` is set. |
| `download_deadline_sec` | u64 | `600` | Whole-cycle deadline for one download attempt. Distinct from any client-level total timeout, which is deliberately left unset (see below). Must be greater than zero when `update_url` is set. |
| `stall_timeout_sec` | u64 | `60` | Abort a download that makes no progress for this long. Must be greater than zero when `update_url` is set. |
| `max_db_bytes` | u64 | `268435456` (256 MiB) | Hard cap enforced *while streaming* the update. Must be greater than zero when `update_url` is set. |

The updater's outbound HTTP client is built through the project's shared `build_outbound_client`
helper, so it goes through the same egress policy (proxy handling, redirect denial, DNS
resolver) as every other gateway-initiated outbound call. The client's own total-request timeout
is deliberately left unset — a 60–80 MB database over a slow mirror link would otherwise abort
every single cycle — and `download_deadline_sec` / `stall_timeout_sec` are what actually bound
the request instead.

The Authorization value is parsed once into a typed HTTP header and marked sensitive before it is
stored. Clones retain that metadata, which provides cooperative structured-`Debug` masking and
selects HTTP/2's never-indexed representation. This metadata is not encryption or memory
zeroization, and it does not promise blanket redaction of serialized HTTP/1 wire logs.

### Database file lifecycle

**The container image ships no database.** An absent `db_path` at first start is the normal
"not ready yet" state, not a failure: the provider stays empty, lookups report `Unavailable`,
and (if `update_url` is set) the background updater fetches the file and swaps it in. Whether
the file survives a pod restart is your deployment's decision — an `emptyDir` re-downloads on
every restart; a PVC or `hostPath` preserves it across restarts.

**Never write `db_path` in place.** `kubectl cp`, an initContainer `cp`, a projected-volume
content update, or any script that truncates and rewrites the file will cause a **SIGBUS** in
the gateway process, because it holds the file memory-mapped. SIGBUS is **not** a Rust panic —
it is not caught by `catch_unwind`, does not run destructors, and kills the process outright,
mid-request, for every in-flight connection on that gateway instance. This is a hard operator
constraint, not a style preference.

The only supported ways to update the database file are:

1. The built-in updater (`update_url` configured), which writes to a `.tmp` file and
   `rename(2)`s it into place — an atomic operation that never disturbs bytes an existing mmap
   is reading.
2. An external writer that does the same: write a **temporary file on the same filesystem** as
   `db_path`, then `rename(2)` it over `db_path`. A cross-filesystem "temp file + rename" is
   *not* atomic (it degrades to copy+unlink) and reintroduces the exact hazard this rule exists
   to avoid — the gateway checks this explicitly at startup for its own updater and refuses to
   start the updater task if the swap directory isn't usable, but it has no way to check an
   external writer's behavior.

**Size the update volume for `max_db_bytes` + 2× the current database size, not 3× `max_db_bytes`.**
During a swap, the volume simultaneously holds: the old file's inode (unlinked from the directory
but still referenced by the gateway's mmap until the last request finishes with it), the
newly-downloaded file, and — if a previous cycle failed partway — a stale `.tmp`. The free-space
precheck matches this: it requires `max_db_bytes` (the `.tmp` being streamed, capped at that
limit) plus 2× the database's *current on-disk size* (the old file, counted twice to cover the
transient double-hold around the rename), falling back to `max_db_bytes` alone when no database
exists yet. Sizing against 3× `max_db_bytes` instead overshoots whenever the live database is
smaller than the cap — the common case — and an undersized-relative-to-that-formula volume fails
every cycle with `reason="disk"` before any request is even issued. The updater does clean up a
stale `.tmp` at startup and at the beginning of every cycle, but that doesn't help if the volume
itself is undersized: a `.tmp` that can't fit fails every subsequent cycle the same way,
permanently, until the volume is resized or the `.tmp` is manually removed.

### Update validation

A downloaded `.tmp` file is validated **structurally only**: the MMDB reader must be able to
open it, `binary_format_version` must be sane, `node_count` must be nonzero, `build_epoch` must
be a sane timestamp, `database_type` must be non-empty, and the search tree must be traversable.
There is **no content-level probe of any kind**, not even an operator-configurable one — no fixed
address is guaranteed to resolve in an arbitrary operator-supplied MMDB (a trimmed regional
mirror, an IPv6-only build, a database that only covers RFC1918 space for internal testing), so
any content probe — hardcoded or operator-supplied — would permanently reject a perfectly valid
database that just doesn't happen to resolve the configured address. A failed validation never
renames; a half-broken database is strictly worse than keeping the old one loaded.

## The `GeoIpLocation` plugin

`GeoIpLocation` shares one config shape across both registrations and has two modes, selected
entirely by whether any rule group is configured:

```yaml
# Parse-only — resolve geo into the access log (HTTP only); decide nothing.
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: geo-log
  namespace: production
spec:
  requestPlugins:
    - type: GeoIpLocation
      config: {}
```

```yaml
# Rule mode — deny/allow by geo dimension.
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: geo-block
  namespace: production
spec:
  requestPlugins:
    - type: GeoIpLocation
      config:
        defaultAction: allow          # allow | deny
        failOpen: false               # REQUIRED once any rule group is configured
        denyRuleGroups:
          - name: block-moscow
            rules:
              - source: DirectPeerIp
                key: city
                values: ["Moscow"]
        allowRuleGroups: []
```

The identical `config` block also registers as an `EdgionStreamPlugins.spec.plugins` entry for
listener (including TLS), TCPRoute, and UDP transport-route enforcement, with one restriction:
`source: RemoteIp` is rejected at L4 (see below).

**Combining geo with other rules is chaining, not embedding.** `GeoIpLocation` carries no
IP-allowlist or header/body predicates, and `IpRestriction`/`RequestRestriction`/
`RequestAccessPolicy`/`GlobalConnectionIpRestriction` carry no geo predicates. "Block this
country unless the source IP is on the office allowlist" is expressed by placing `GeoIpLocation`
and the IP plugin as **separate, ordered entries** on the same plugin list:

```yaml
spec:
  requestPlugins:
    - type: IpRestriction
      config:
        allow:
          - name: office-network
            description: Office clients exempt from the geo rule
            cidrs: ["10.0.0.0/8"]
    - type: GeoIpLocation
      config:
        defaultAction: allow
        failOpen: false
        denyRuleGroups:
          - name: block-ru
            rules:
              - source: DirectPeerIp
                key: country
                values: ["RU"]
```

### Rule shape

- **`source`**: `DirectPeerIp` is the immediate TCP peer — the previous hop, never rewritten,
  unforgeable, valid at both L4 and L7. `RemoteIp` is the client resolved from
  `X-Forwarded-For` against the trusted-proxy set — **L7 only**; a rule using `RemoteIp` in an
  L4 `GeoIpLocation` config is a hard admission error, since L4 has no HTTP headers to resolve a
  forwarded-for chain from. See [`RemoteIp` is spoofable](#remoteip-is-spoofable-directpeerip-is-not).
- **`key`**: `cidr` matches the address itself against one or more CIDRs/bare IPs (a bare IP is
  shorthand for its `/32`/`/128`) and needs no GeoIP database at all — see
  [CIDR-only rules](#cidr-only-rules-never-touch-failopen). `country` / `continent` /
  `subdivision` are ISO codes, normalized to **uppercase** at compile time; an unrecognized code
  is a **WARN**, not a rejection (lenient-admission policy). `city` is free text, matched against
  `names["en"]`, case-sensitive — see [`city` matches the English name only](#city-matches-the-english-name-only).
- **`values`**: must be **non-empty** — an empty list is rejected at admission. A rule with no
  values expresses no criterion, and reading it as "matches everything" would turn a deny group
  into a total outage and an allow group into an open whitelist. To drop a criterion, delete the
  rule. A dimension the resolved record cannot supply makes a rule simply **not match** — there
  is no separate `onMissing` knob to configure; see
  [Absent dimensions](#absent-dimensions-are-ordinary-rule-misses).

### Unknown fields are rejected, not ignored

Every `GeoIpLocation` config struct is strict: a field name this plugin does not recognize is a
hard admission error naming the offending key, not a silently ignored line.

This is deliberate, and stricter than most other plugins. A geo config is a security control, and
the ways it can be mistyped all fail *open*:

| Typo | What it used to do |
|---|---|
| `denyRuleGroup:` (singular) | The whole rule list vanished, leaving an empty config — i.e. parse-only mode. Every request was allowed, `failOpen: false` stopped being required, and the resource still reported `Accepted: True`. |
| `valuez:` inside a rule | `values` defaulted to empty, which used to mean "matches everything" — a total outage in a deny group, an open whitelist in an allow group. |

Neither is detectable from the outside: the resource is accepted and traffic flows. The
EdgionPlugins CRD models plugin `config` as `x-kubernetes-preserve-unknown-fields`, so the API
server forwards unknown keys untouched — schema strictness here is the only gate that can catch
them.

The practical consequence: **an empty `config: {}` now means parse-only because someone wrote it
that way**, never because a rule silently failed to parse.

### Rule groups: AND within a group, OR across groups

```yaml
denyRuleGroups:
  - name: cn-beijing
    rules:
      - source: DirectPeerIp
        key: country
        values: ["CN"]
      - source: DirectPeerIp
        key: city
        values: ["Beijing"]
```

Every rule in one group is ANDed; `denyRuleGroups`/`allowRuleGroups` are each OR-ed internally.
The plugin's judgement (`decide()`):

1. Any `denyRuleGroups` group matches → **Deny**.
2. `allowRuleGroups` is non-empty and no group matches → **Deny** (whitelist-miss semantics).
3. Otherwise → `defaultAction`.

**Pair `city` and `subdivision` with `country`.** Neither is globally unique: "Cambridge" exists
in several countries, and a bare `subdivision: ["CA"]` matches California *and* any Canadian
subdivision coded `CA`.

**L4 has no dedicated `city` restriction, but city rules are conventionally HTTP-only.** At L4
there is only a connection, with no request attributes to compose against, so fine-grained
geographic rules there are hard to operate and easy to over-block; city-level decisions belong at
the HTTP layer.

### `failOpen` — the database-unavailable and lookup-failure disposition

`failOpen` is **required** exactly when `denyRuleGroups`/`allowRuleGroups` is non-empty (rule
mode) and **rejected** in parse-only mode — the same validation runs on the controller ingest
path and the gateway construction path, so the two can never disagree. It governs **only this
plugin's own decision** when the GeoIP database itself is unavailable
(`GeoLookup::Unavailable`) or a loaded-database lookup fails (`GeoLookup::Failed`); it never
reaches into any other plugin on the chain — that isolation is the entire reason geo lives in
its own plugin rather than as predicates embedded in another plugin's config.

| Condition | Governed by |
|---|---|
| No database loaded at all (`GeoLookup::Unavailable`) | `failOpen` |
| Loaded database, but this address could not be decoded (or the reader panicked) (`GeoLookup::Failed`) | `failOpen` |
| Database loaded but no record for this address, or the record lacks this dimension | ordinary rule-miss (an absent dimension simply doesn't match a constrained rule) |

- **`failOpen: false`** — **HTTP**: the plugin denies the request (`403`, JSON body).
  **L4**: the connection is denied and `edgion_geoip_denied_total` increments with
  `reason="db_unavailable"` (no database) or `reason="lookup_failed"` (loaded database, this
  lookup failed).
- **`failOpen: true`** — the plugin has **no opinion**: HTTP passes the request through
  unchanged; L4 lets the connection continue to the next stream plugin.

An IPv4-only (or otherwise non-covering) database that returns an MMDB input error for a native
IPv6 address is `Failed`, not `NotFound`, so it follows `failOpen` rather than ordinary
rule-miss. Official GeoLite2-class databases include IPv6. IPv4-mapped peers are normalized to
IPv4 before lookup.

**What each disposition costs, concretely, during the cold-start window** (a fresh rollout on an
`emptyDir` with no database yet, or any period where the updater hasn't succeeded even once):

- `failOpen: false` on a security-critical rule (e.g. "block this country") means every request
  matching that rule group is **rejected** until the database loads — you trade availability for
  the guarantee that the rule was never silently bypassed.
- `failOpen: true` means every request is **admitted** as if `GeoIpLocation` weren't on the chain
  at all, until the database loads — you trade enforcement for availability during the window.

There is no third option and no default; you must write one explicitly whenever any rule group is
configured, and you may not write one in parse-only mode.

#### CIDR-only rules never touch `failOpen`

A rule set made entirely of `key: cidr` rules needs no GeoIP database at all — CIDR matching
compares the peer address directly, with no MMDB lookup. Both the HTTP and L4 plugins detect this
per-source (`source_needs_geo_facts`) and skip the database lookup entirely for that source when
none of its rules need a resolved dimension, so `failOpen` is never consulted for a pure IP
allow/deny list — it stays correct regardless of whether the unrelated country database happens
to be loaded.

### `RemoteIp` is spoofable; `DirectPeerIp` is not

`RemoteIp` resolves from an address that is **derived from `X-Forwarded-For`**, which is
client-controlled input. It is only trustworthy when the deployment genuinely terminates behind
a trusted proxy — i.e., a `RealIp` plugin is configured with a `trustedIps` set that actually,
tightly matches your real proxy/CDN egress ranges, and the connection's TCP peer is inside that
set. If those conditions don't hold, `RemoteIp` is just a clone of the effective L4 peer address, and
a `RemoteIp` rule degrades to (harmlessly) the same value a `DirectPeerIp` rule would read.

The dangerous configuration is a `trustedIps` set scoped far too broadly — the extreme case being
a default route:

```yaml
# ANTI-PATTERN — do not do this
- type: RealIp
  config:
    trustedIps:
      - name: everyone
        cidrs: ["0.0.0.0/0"]
```

With this in place, **any** client can send `X-Forwarded-For: 1.1.1.1` and be treated as
originating from wherever `1.1.1.1` geolocates, completely bypassing a deny rule sourced on
`RemoteIp`. This is a realistic misconfiguration behind a CDN whose published egress ranges are
incomplete or change — an operator broadens `trustedIps` "to be safe" and unknowingly hands
every client full control over the geo decision.

> **Note on automated detection.** There is currently **no ingest-time warning** that
> cross-checks a `RemoteIp` `GeoIpLocation` rule against the listener's `RealIp`/`trustedIps`
> configuration. The trustworthiness of `RemoteIp` is entirely the operator's responsibility to
> verify by inspecting the `RealIp` plugin config for the same route/gateway. If your deployment
> does not genuinely terminate behind a trusted, tightly-scoped proxy, prefer `DirectPeerIp` for
> any rule that matters for security — it always reads the raw TCP peer and cannot be influenced
> by request headers. `RemoteIp` is rejected outright in L4 `GeoIpLocation` configs, so this
> concern only applies to HTTP.

### No `registered_country` fallback

There is deliberately no fallback to `registered_country` or `represented_country` when
`country.iso_code` is absent. Satellite ISPs, anycast address blocks, some anonymizing networks,
and certain military/embassy allocations have a populated `registered_country` (the
*registrant*) but no resolvable `country` (the *client's actual location*) — conflating the two
would make one `country` rule mean two different things depending on the client. Those clients
resolve to `country: None` and a constrained `country` rule simply never matches them. If your
traffic includes a meaningful share of satellite or anycast clients, expect some of them to be
permanently unresolved by design, not by bug.

Any other absence works the same way: no MMDB record at all for the address, and a record that
exists but lacks this specific dimension (very common for `subdivision`/`city` — many
city/subdivision-carrying records only populate `geoname_id` + names, with no `iso_code`, and
`city.names` frequently lacks an `"en"` key for less-common places) both simply fail to match a
constrained rule.

### `city` matches the English name only

`city` is populated from `city.names["en"]` — the English localized name, never a translated
one. A city rule matching `values: ["Pekín"]` (or any other non-English spelling) will never
match, because the lookup result is always the English form. `city` and `subdivision` are also
**not globally unique** — "Cambridge" exists in multiple countries, and a bare
`subdivision: ["CA"]` matches both California and any Canadian province coded `CA`. Always pair
`city` or `subdivision` with a `country` rule in the same AND group (see the rule-group example
above).

### Absent dimensions are ordinary rule misses

A parsed MMDB record is uniform regardless of which product built the database. `city`/
`subdivision` is `Some(...)` when the record carries that data and `None` when it doesn't — for
any of three indistinguishable reasons (the database never carries that dimension at all, this
specific record lacks it, or the address didn't resolve to a record). All three are ordinary
"this constrained rule doesn't match" — there is no separate absence-handling knob the way other
plugins have `onMissing`; the non-empty-`values` / dimension-absent-is-no-match rules above (see
[Rule shape](#rule-shape)) already cover it.

**There is no "database tier" concept.** An earlier draft of this feature modeled MaxMind
product lines as falling into a "City tier" vs. "Country tier", and warned when a `city`/
`subdivision` rule was compiled against what looked like a Country-only database. That check was
removed: `database_type` names a product family as free text (`GeoLite2-City`,
`GeoIP2-Enterprise`, a vendor's own OEM name, ...), and there is no reliable way to classify an
arbitrary string into "has city data" versus "doesn't" — a database that genuinely carries city/
subdivision granularity but whose type string doesn't contain a recognized keyword would produce
a **false** warning telling the operator their working rule was inert, which is worse than no
warning at all. There is no ingest-time or gateway-log signal for this, because there is nothing
gateway-side can validate: whether a given `city`/`subdivision` rule will ever produce a value
depends entirely on what database the operator loads, which can change at any update cycle.
Check the access log's `geo` block for the fields your loaded database actually resolves against
your provider's documentation, and shape your rule groups accordingly.

## L4 (TCP/UDP/TLS) registration

`GeoIpLocation` registers in `EdgionStreamPlugins.spec.plugins`, whose runtime is used at two
explicit lifecycle layers:

- `Stage1`: listener `ConnectionFilter`, before route matching or TLS.
- `Tcp`: TCPRoute connection admission and the UDP transport-route session-creation path.

GeoIP is not admitted in `tlsRoutePlugins`, which allows only `IpRestriction`.
For TLS traffic, bind the resource to its Gateway listener, using the Gateway annotation
`edgion.io/edgion-stream-plugins: "geo-policy"` (a bare resource name in the Gateway namespace).
This enforces GeoIP before the TLS handshake and route matching for every connection on
that listener. A TLSRoute reference to a nonempty Stage-1-only resource rejects connections;
it does not execute GeoIP.

The L4 config is the same structure as the HTTP one, with two layer-specific admission rules:

| Rule | Why |
|---|---|
| `source: RemoteIp` is rejected | L4 has no `X-Forwarded-For` to resolve a real client from. Use `DirectPeerIp`. |
| **Parse-only (no rule groups) is rejected** | L4 has no access log and `StreamContext` carries no logging sink, so a resolved location has nowhere to go — the plugin would provably do nothing. An L4 entry must carry at least one `denyRuleGroups`/`allowRuleGroups` group. |

The second rule is the reason observe-without-enforcing is an **HTTP-layer capability**: at L7 a
parse-only entry fills the access log's `geo` block, which is a real product. At L4 the same
config would be a security control that silently does not run, so it is refused at admission
rather than accepted as a documented no-op.

```yaml
apiVersion: edgion.io/v1
kind: EdgionStreamPlugins
metadata:
  name: geo-block
  namespace: production
spec:
  plugins:
    - type: GeoIpLocation
      config:
        defaultAction: allow
        failOpen: false
        denyRuleGroups:
          - name: block-ru
            rules:
              - source: DirectPeerIp
                key: country
                values: ["RU"]
```

`source: RemoteIp` is a hard admission error in an L4 config — rejected identically at controller
ingest and at gateway construction (both call the same shared validation function) — since L4 has
no `X-Forwarded-For` to resolve a real client from.

### Always the physical transport peer

Every L4 invocation receives `StreamContext.direct_peer_ip`, the physical transport peer that is
never rewritten by PROXY protocol. The lifecycle layer changes where the chain runs, not the
meaning of `DirectPeerIp`: `Stage1` and `Tcp` both evaluate that same transport identity.

### No readiness gating; the operator owns the cold-start window

A fresh rollout with no database yet (typical on an `emptyDir` volume) does **not** hold back the
readiness probe. `failOpen` is how you declare what should happen instead: `true` accepts a
window of unenforced geo policy, `false` accepts a window of rejected traffic. The pre-first-load
backoff (see below) bounds that window to seconds, not the full `retry_interval_sec`, and
`edgion_geoip_db_loaded` is the metric to alert on for "is this still going on."

### L4 denial signals depend on the lifecycle layer

A `Stage1` listener denial is reduced to the `ConnectionFilter` boolean result. Its only GeoIP
signal is `edgion_geoip_denied_total{layer="stage1",reason=...}`; there is no protocol context or
canonical connection-log channel at that point.

The `Tcp` and `Tls` route paths retain their canonical TCP, UDP, or TLS connection logs. They
collapse a GeoIP denial to a generic stream-plugin denial before logging, so the log contains no
GeoIP facts or sensitive plugin reason; the bounded counter carries the `tcp` or `tls` layer and
reason. HTTP denials remain ordinary request diagnostics and access-log entries.

## Access log

There is no global switch: geo is entirely plugin-driven, and the access log only ever reports a
slot that the HTTP `GeoIpLocation` plugin actually queried for the current request. Adding a
`GeoIpLocation` entry to a route's `requestPlugins` — parse-only or rule mode — gives that
request's access log entry a `geo` object with the slot(s) that were queried:

```json
"geo": {
  "remote": { "status": "found", "country": "CN", "continent": "AS", "city": "Beijing" },
  "direct_peer": { "status": "found", "country": "US" }
}
```

Each slot (`remote` = the `RemoteIp`-side resolution, `direct_peer` = the `DirectPeerIp`-side
resolution) is an independent, per-request `OnceLock`. The access log reads whichever slots the
plugin already resolved earlier in the request — it never triggers a lookup of its own. A route
with no `GeoIpLocation` entry at all therefore costs **zero** MMDB lookups. Parse-only mode
(`config: {}`) always resolves both slots — that is its entire purpose; rule mode resolves only
the sources/dimensions its configured rules actually use (a CIDR-only rule set, for instance,
needs no resolved dimensions at all for the sources it covers). Each slot carries a bounded
`status`: `found`, `not_found`, `unavailable`, or `lookup_failed`. `lookup_failed` means the
database was loaded but this address could not be decoded (or the reader panicked); it never
includes decoder text, an IP, or a database path.

### Coordinates

There is no switch for latitude/longitude: a `geo` slot emits `coord` whenever the resolved
record actually carries coordinates, and omits it otherwise — the same "reads whatever the
plugin already resolved" rule as the rest of the `geo` object, with no separate gate on top. When
present, coordinates are emitted as a single `coord` string and pass through the same masking
treatment as `direct_peer_ip`/`remote_ip`.

**There is no per-key unmasking for `coord`.** Masking is controlled entirely by the
process-global sensitive-log mode — an all-or-nothing switch. The per-`LogExtSource` allowlist
mechanism that lets some ext-log fields opt out of masking does not apply here; `direct_peer_ip` and
`remote_ip` aren't individually unmaskable today either, and `coord` follows the same rule.

## Metrics

| Metric | Type | Labels | Meaning |
|---|---|---|---|
| `edgion_geoip_db_loaded` | Gauge | none | `1` when a GeoIP database is loaded, `0` otherwise. Alert on `== 0` while geo rules are configured — this is the primary "security control isn't running" signal. |
| `edgion_geoip_db_build_epoch_seconds` | Gauge | none | Build timestamp of the loaded database. Alert on staleness. |
| `edgion_geoip_update_failed_total` | Counter | `reason` (`fetch` \| `encoding` \| `size_cap` \| `stall` \| `validation` \| `rename` \| `disk` \| `deadline`) | Background *update-cycle* failures, by class. `disk` is the free-space precheck refusing to start (no request was ever issued); `deadline` is the whole cycle exceeding `download_deadline_sec` despite steady progress, distinct from `stall` (no progress within `stall_timeout_sec`) — the two call for opposite fixes. |
| `edgion_geoip_lookup_panic_total` | Counter | none | A per-request MMDB lookup panicked (caught) and was reported as `Failed`. Distinct from `edgion_geoip_update_failed_total` above: this is a per-request event on the query path, not an update-cycle failure. Decode-without-panic failures share the `Failed` outcome but do not increment this counter. |
| `edgion_geoip_denied_total` | Counter | `layer` (`stage1` \| `tcp`), `reason` (`deny_country` \| `allow_miss` \| `db_unavailable` \| `lookup_failed`) | L4 decisions denied by `GeoIpLocation`. `stage1` is listener admission (including TLS), and `tcp` covers TCPRoute and UDP transport-route admission. The metric never carries a client address; HTTP denials are deliberately absent because they already appear in the access log. |

## Multi-replica update behavior, jitter, and the `.meta` validator

The updater's request cycle sends a conditional `GET` (`If-None-Match` /
`If-Modified-Since`) only when the local validator context is eligible. A valid `304` sleeps until
the next interval without touching disk; an untrusted `304` is a `fetch` failure, clears the
validators, uses cold backoff, and makes the next request unconditional. A `200` with a
non-`identity` `Content-Encoding` is rejected loudly (the updater does not decompress — point
`update_url` at a plain MMDB, not a `.mmdb.gz`); a `200` with `identity` encoding streams to
`<db_path>.tmp` under the `max_db_bytes`/`stall_timeout_sec`/`download_deadline_sec` bounds; on
success, `rename` → re-mmap → swap → persist the validator to `<db_path>.meta`.

**Backoff has two regimes.** Before the *first* successful load, retries use a short escalating
backoff (1s, doubling, capped at 30s) — `retry_interval_sec`'s default of 600s is tuned for
steady state and would otherwise leave a cold-started pod effectively unprotected for ten
minutes. After the first success, ordinary failures use `retry_interval_sec` plus jitter.

**Jitter applies to the very first fetch too, not only to retries.** Without it, a 200-pod
rolling restart issues 200 simultaneous unconditional GETs against the mirror. Jitter alone
doesn't make those conditional, though — that's the job of the `.meta` file below.

**`<db_path>.meta` persists the ETag/Last-Modified validator** used to make requests conditional.
A validator is eligible only while `db_path` is a regular file and the provider has an active
database. If both are usable but `.meta` is absent or unreadable, the updater seeds
`If-Modified-Since` from the database file's mtime. A valid empty sidecar deliberately suppresses
that fallback and keeps the next request unconditional.

If either the durable file or active provider is missing, the updater clears both its in-memory
validator and the sidecar before constructing an unconditional request. A `304` is accepted only
when that request actually carried a validator and both local conditions still hold when the
response arrives. Any other `304` is a bounded `fetch` failure: validators are cleared, cold
backoff is used, and the next request is unconditional. Thus losing `.meta` alone while a usable
database remains produces an mtime-based conditional request; losing the usable local database
state produces an unconditional request.

## Troubleshooting

| Symptom | Likely cause | Fix |
|---|---|---|
| `edgion_geoip_db_loaded` stays `0` even though your YAML clearly sets `geoip.db_path` | `geoip:` is at the document root instead of nested under `gateway:` | Move it under `gateway:` — see [the top-level nesting trap](#-the-single-most-common-way-to-get-this-wrong) |
| Config load fails with an unknown-field error mentioning a `geoip` sub-key | A camelCase typo inside the `geoip` block (e.g. `dbPath`) | Use snake_case — `db_path`, `update_url`, etc. |
| `edgion_geoip_db_loaded` stays `0` after a fresh deploy | Normal at first start — no bundled database, and the updater hasn't completed its first cycle yet | Wait for the first successful cycle, or check `edgion_geoip_update_failed_total{reason}` for a stuck failure |
| Gateway process dies with no panic log, mid-request, shortly after a database file was touched externally | Something wrote `db_path` in place instead of rename-swapping it — SIGBUS | Stop whatever wrote in place; only the built-in updater or a same-filesystem temp-file-plus-rename writer are supported |
| Updater cycles fail permanently with `reason="disk"` (or an `ENOSPC`-flavored error) | Update volume undersized for a swap (needs `max_db_bytes` for the `.tmp` + 2× the current database size for the old inode/rename double-hold) | Resize the volume per the formula above, not to 3× `max_db_bytes` |
| A `country` + `city` rule group never matches anything | The loaded database's records don't carry city data for this rule — see [no "database tier" concept](#absent-dimensions-are-ordinary-rule-misses) | Check the access log's `geo` block against your provider's documentation for what the loaded database actually resolves; drop the `city`/`subdivision` rule if it's structurally never going to match |
| A `city` rule with a translated-language value never matches | `city` only ever compares against `names["en"]` | Use the English city name |
| A blacklist-mode rule sourced on `RemoteIp` is trivially bypassed by any client | `RealIp`'s `trustedIps` is scoped too broadly (e.g. `0.0.0.0/0`) — `RemoteIp` degrades to attacker-controlled input | Scope `trustedIps` to your actual proxy egress ranges, or switch the rule to `DirectPeerIp` |
| L4 config rejected at ingest/compile mentioning `RemoteIp` | `source: RemoteIp` is a hard admission error in an L4 `GeoIpLocation` config | Use `source: DirectPeerIp` — L4 has no `X-Forwarded-For` to resolve a real client from |
| A listener-stage geo denial has no log line | Expected — `Stage1` has only the connection-filter boolean and bounded metric | Use `edgion_geoip_denied_total{layer="stage1",reason=...}` |
| A TCP/UDP route-stage geo denial log lacks GeoIP details | Expected — route paths keep canonical protocol logs but replace GeoIP facts and plugin reasons with a generic stream-plugin denial | Correlate the protocol log with `edgion_geoip_denied_total{layer="tcp",reason=...}`; TLS listener denials use the Stage1 metric-only path above |
| A "block this country unless it's on the office allowlist" rule doesn't behave as expected | Check the ordering of `IpRestriction`/other admission plugins relative to `GeoIpLocation` on the plugin list — composition is by chain order, not by a single merged plugin | Reorder the plugin-list entries; there is no cross-plugin exemption semantics to configure |

## Related documents

- [Gateway Configuration](../configuration/gateway-config.md) — full `GatewayConfig` schema reference, including `geoip`
- [Real IP](../../user-guide/edgion-plugins/real-ip.md) — `trustedIps` configuration that governs whether `RemoteIp` is trustworthy
- [Metrics](../observability/metrics-inventory.md) — full metrics inventory, including the five `edgion_geoip_*` series
