# GeoIP Restriction

> **Gateway-local feature.** GeoIP configuration lives in the gateway's local startup config
> (not a CRD) because the database, its memory map, and the background updater are one
> process-wide resource, not something that can vary per named `EdgionGatewayConfig` instance.
> Changing any `geoip` field requires a pod restart.

GeoIP lets you restrict traffic by the geographic origin of the client and enriches the access
log with the resolved country/continent/city. It extends two existing mechanisms rather than
adding new plugins:

- **HTTP** — two new `RestrictionSource` values (`ClientGeo`, `DirectGeo`) on the existing
  `RequestRestriction` and `GlobalAccessControl` plugins.
- **L4 (TCP/TLS)** — an optional `geo` block on the existing `GlobalConnectionIpRestriction`
  stream plugin.

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
| `update_auth_header_env` | string | none | Name of an environment variable whose value is sent verbatim as the `Authorization` header. In Kubernetes, inject it from a Secret via `secretKeyRef`. |
| `update_interval_sec` | u64 | `86400` | Steady-state interval between update cycles (after the first successful load). |
| `retry_interval_sec` | u64 | `600` | Steady-state retry backoff after a failed cycle, **once a database has already loaded successfully once**. Not used for the very first fetch — see [cold-start backoff](#multi-replica-update-behavior-jitter-and-the-meta-validator). |
| `connect_timeout_sec` | u64 | `10` | TCP connect timeout for the update request. |
| `download_deadline_sec` | u64 | `600` | Whole-cycle deadline for one download attempt. Distinct from any client-level total timeout, which is deliberately left unset (see below). |
| `stall_timeout_sec` | u64 | `60` | Abort a download that makes no progress for this long. |
| `max_db_bytes` | u64 | `268435456` (256 MiB) | Hard cap enforced *while streaming* the update. |

The updater's outbound HTTP client is built through the project's shared `build_outbound_client`
helper, so it goes through the same egress policy (proxy handling, redirect denial, DNS
resolver) as every other gateway-initiated outbound call. The client's own total-request timeout
is deliberately left unset — a 60–80 MB database over a slow mirror link would otherwise abort
every single cycle — and `download_deadline_sec` / `stall_timeout_sec` are what actually bound
the request instead.

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

## HTTP-layer rules

`RestrictionSource` on the `RequestRestriction` and `GlobalAccessControl` plugins gained two
variants: `ClientGeo` and `DirectGeo`. Both plugins share the same predicate model
(`request_match.rs`), so geo rules work identically in either plugin.

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: geo-block
  namespace: production
spec:
  requestPlugins:
    - type: RequestRestriction
      config:
        mode: Blacklist
        ruleGroups:
          - name: block-ru
            rules:
              - source: ClientGeo
                key: country
                values: ["RU"]
        failOpen: false
```

- **`source`**: `ClientGeo` reads the resolved real-client address (after `X-Forwarded-For` /
  PROXY protocol resolution — the same address a configured `RealIp` plugin exposes). `DirectGeo`
  reads the direct TCP peer address, unaffected by any header.
- **`key`**: one of `country` | `continent` | `city` | `subdivision`. Required on every geo rule;
  a missing or misspelled key is a schema error and is rejected at ingest.
- **`values`**: for `country`/`continent`, values and lookup results are both normalized to
  **uppercase**, and checked against the static ISO 3166-1 alpha-2 / continent-code table — an
  unrecognized code is a **WARN**, not a rejection (lenient-admission policy: the rule is still
  applied as written). `caseSensitive` is **rejected** outright on `country`/`continent` keys
  (the case-insensitive matcher folds to lowercase, the opposite direction from this
  normalization, so allowing the flag would silently produce a rule that can never match). `city`
  and `subdivision` are free text and keep the plugin's normal `caseSensitive` semantics.
- **`regex` and `secretRef` are rejected** on geo sources. The lookup result is always an exact
  code — allowing `regex` would compile cleanly and then simply never match anything, since
  nothing produces the kind of free-form string a regex is useful against.
- **`failOpen`** lives on the *plugin config* (`RequestRestrictionConfig.failOpen` /
  `GlobalAccessControlConfig.failOpen`), not on the individual rule. It is **required**, with no
  default, precisely when the plugin has at least one `ClientGeo`/`DirectGeo` rule anywhere in any
  group (or, for `GlobalAccessControl`, any profile) — and is **rejected** when the plugin has
  none. See [`failOpen`](#failopen-the-database-unavailable-disposition) below for what it means,
  why it is plugin-level rather than per-rule, and what it costs.

### No `registered_country` fallback

There is deliberately no fallback to `registered_country` or `represented_country` when
`country.iso_code` is absent. Satellite ISPs, anycast address blocks, some anonymizing networks,
and certain military/embassy allocations have a populated `registered_country` (the
*registrant*) but no resolvable `country` (the *client's actual location*) — conflating the two
would make one `country` rule mean two different things depending on the client. Those clients
resolve to `country: None` and fall through to `onMissing` (see below). If your traffic includes
a meaningful share of satellite or anycast clients, expect some of them to be permanently
unresolved by design, not by bug.

`onMissing` governs two distinct absence cases the same way: no MMDB record at all for the
address, and a record that exists but lacks this specific dimension (very common for
`subdivision`/`city` — many city/subdivision-carrying records only populate `geoname_id` +
names, with no `iso_code`, and `city.names` frequently lacks an `"en"` key for less-common
places).

### `city` matches the English name only

`city` is populated from `city.names["en"]` — the English localized name, never a translated
one. A city rule matching `values: ["北京"]` (or any other non-English spelling) will never
match, because the lookup result is always the English form. `city` and `subdivision` are also
**not globally unique** — "Cambridge" exists in multiple countries, and a bare
`subdivision: ["CA"]` matches both California and any Canadian province coded `CA`. Always pair
`city` or `subdivision` with a `country` rule in the same AND group:

```yaml
ruleGroups:
  - name: cn-beijing
    rules:
      - source: ClientGeo
        key: country
        values: ["CN"]
      - source: ClientGeo
        key: city
        values: ["Beijing"]
failOpen: false
```

### `failOpen` — the database-unavailable disposition

`onMissing` cannot express "fail closed": in blacklist mode a false predicate means *allow*, so
"the database isn't loaded" and "block everything until it is" cannot both be expressed with
`onMissing`. `failOpen` splits this out as a separate, required, **plugin-level** short circuit —
it lives on `RequestRestrictionConfig`/`GlobalAccessControlConfig`, not on the individual rule:

| Condition | Governed by |
|---|---|
| No database loaded at all (`GeoLookup::Unavailable`) | the plugin's `failOpen` |
| Database loaded but no record for this address, or the record lacks this dimension | `onMissing` |

Earlier iterations of this feature put `geoFailOpen` on each rule, but the runtime behavior was
always plugin-wide: any single rule declaring `false` short-circuits the *whole plugin*
regardless of `mode` or which other group it's in, so a per-rule value could only ever agree with
itself across every geo rule in the plugin — there is no scenario where "rule A fails open, rule B
fails closed" means anything coherent within one plugin. The field now lives where its effect
actually lives: one `failOpen` per plugin, required exactly when that plugin has at least one geo
rule (in any rule group, or for `GlobalAccessControl`, in any profile) and rejected otherwise.

- **`failOpen: false`** — the **plugin short-circuits straight to rejection**, using its
  configured `status`/`message`, **regardless of `mode` (Blacklist or Whitelist) and regardless
  of any other rule group.** This is exactly why it's a separate plugin-level switch rather than
  a predicate outcome: a "false" predicate result would mean *allow* in blacklist mode, which is
  the opposite of what an unavailable security control should default to.
- **`failOpen: true`** — every geo predicate is dropped from its AND group entirely, exactly like
  `onMissing: Skip`.

Either way, an `EdgionStatus` entry records the event in the access log, so "geo enforcement
silently isn't running" is at least visible per-request, even though it can't reach a CRD status
condition — `RequestRestriction`/`GlobalAccessControl` compile inside the gateway process,
`Accepted` conditions are written only by the controller, and the controller↔gateway sync
protocol has no data-plane→control-plane channel to carry a gateway-process runtime fact back.

**What each disposition costs, concretely, during the cold-start window** (a fresh rollout on an
`emptyDir` with no database yet, or any period where the updater hasn't succeeded even once):

- `failOpen: false` on a plugin with a security-critical rule (e.g., "block this country") means
  every request matching that rule group is **rejected** until the database loads — you are
  trading availability for the guarantee that the rule was never silently bypassed.
- `failOpen: true` means every request is **admitted** as if the geo predicate weren't there
  at all, until the database loads — you are trading enforcement for availability during the
  window. If the rest of the group is entirely geo predicates with `onMissing: Skip` in
  **Whitelist mode**, see the all-Skip hazard below — that specific combination produces the
  opposite of what `failOpen: true` alone would suggest.

There is no third option and no default; you must write one explicitly on every plugin that has
at least one geo rule, and you may not write one on a plugin that has none.

### `ClientGeo` is spoofable; `DirectGeo` is not

`ClientGeo` resolves from an address that is **derived from `X-Forwarded-For`**, which is
client-controlled input. It is only trustworthy when the deployment genuinely terminates behind
a trusted proxy — i.e., a `RealIp` plugin is configured with a `trustedIps` set that actually,
tightly matches your real proxy/CDN egress ranges, and the connection's TCP peer is inside that
set. If those conditions don't hold, `remote_addr` is just a clone of the TCP peer address, and
`ClientGeo` degrades to (harmlessly) the same value as `DirectGeo`.

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
originating from wherever `1.1.1.1` geolocates, completely bypassing a `ClientGeo` blacklist
rule. This is a realistic misconfiguration behind a CDN whose published egress ranges are
incomplete or change — an operator broadens `trustedIps` "to be safe" and unknowingly hands
every client full control over the geo decision.

> **Note on automated detection.** Unlike several of the other pitfalls on this page, there is
> currently **no ingest-time warning** that cross-checks a `ClientGeo` rule against the
> listener's `RealIp`/`trustedIps` configuration. The trustworthiness of `ClientGeo` is entirely
> the operator's responsibility to verify by inspecting the `RealIp` plugin config for the same
> route/gateway. If your deployment does not genuinely terminate behind a trusted, tightly-scoped
> proxy, prefer `DirectGeo` for any rule that matters for security — it always reads the raw TCP
> peer and cannot be influenced by request headers.

### `GlobalAccessControl`'s `ipSource` naming inversion

If you use geo rules inside `GlobalAccessControl` alongside its own IP/CIDR preconditions, be
aware that `GlobalAccessControl`'s `ipSource` enum is **inverted** relative to the `ClientGeo`/
`DirectGeo` naming: `ipSource: ClientIp` resolves to the real client address (XFF-derived, the
*same* address `ClientGeo` reads), while `ipSource: RemoteAddr` resolves to the raw TCP peer
(the *same* address `DirectGeo` reads). Pairing `ipSource: ClientIp` with a `DirectGeo` rule (or
`ipSource: RemoteAddr` with a `ClientGeo` rule) makes one AND group decide from two different
addresses, silently. Ingest emits a **WARN** (`GlobalAccessControl ipSource/geo disagreement`)
when a group's `ipSource` and its geo rule's source disagree this way, so this specific
combination — unlike the `ClientGeo`/`trustedIps` one above — does get flagged automatically.

### There is no "database tier" concept — a missing dimension is ordinary absence

Earlier drafts of this feature modeled MaxMind product lines as falling into a "City tier" vs.
"Country tier", and warned when a `city`/`subdivision` rule was compiled against what looked
like a Country-only database. That check was removed: `database_type` names a product family
as free text (`GeoLite2-City`, `GeoIP2-Enterprise`, a vendor's own OEM name, ...), and there is
no reliable way to classify an arbitrary string into "has city data" versus "doesn't" — a
database that genuinely carries city/subdivision granularity but whose type string doesn't
contain a recognized keyword produced a **false** warning telling the operator their working
rule was inert, which is worse than no warning at all.

The honest model: a parsed record is uniform regardless of which product built the database.
`city`/`subdivision` is `Some(...)` when the record carries that data and `None` when it
doesn't — for any of three indistinguishable reasons (the database never carries that dimension
at all, this specific record lacks it, or the address didn't resolve to a record). All three are
ordinary "value absent", governed by `onMissing` exactly like a missing header or cookie:

- `onMissing: NoMatch` (the default) — a `country` + `city` AND group where `city` never
  resolves silently never matches that group. In Blacklist mode, the restriction is
  **effectively inert** for every client, with no error anywhere obvious.
- `onMissing: Skip` — the same group silently widens to country-only instead.
- `onMissing: Match` — the same group matches every client on the country predicate alone,
  since the missing `city` value counts as a hit.

There is no ingest-time or gateway-log signal for this, because there is nothing gateway-side
can validate: whether a given `city`/`subdivision` rule will ever produce a value depends
entirely on what database the operator loads, which can change at any update cycle. Check the
access log's `geo` block for the fields your loaded database actually resolves against your
provider's documentation, and choose `onMissing` deliberately for every `city`/`subdivision`
rule rather than relying on the default.

**All-Skip whitelist denial storm.** In `mode: Whitelist`, a rule group made **entirely** of geo
predicates with `onMissing: Skip`, evaluated while the database isn't loaded yet, produces
`considered == 0` for that group — which the matcher treats as "no group matched", so **every
request is denied** with a 403 and no obvious diagnostic (the group technically had rules, they
were all just skipped). Note that `failOpen: false` reaches its short-circuit *before* this
case and rejects with a recorded reason — the better, diagnosable outcome. It is specifically
`failOpen: true` combined with `onMissing: Skip` on every rule in an all-geo Whitelist group
that produces this silent 403 storm during the cold-start window.

## L4 (TCP/TLS) geo restriction

`GlobalConnectionIpRestriction` gains an optional `geo` block per profile — a nested,
explicitly-opted-in structure (not two implicitly-empty lists) so that "does this profile use
geo at all" is unambiguous:

```yaml
profiles:
  main:
    defaultAction: Deny
    allow:
      - name: office
        cidrs: ["10.0.0.0/8"]
    geo:
      allowCountries: ["CN", "HK"]
      denyCountries: ["RU"]
      failOpen: false        # REQUIRED — no default
```

At least one of `allowCountries` / `denyCountries` must be non-empty; an empty `geo` block is
rejected. `failOpen` has no default, for the same reason as HTTP's plugin-level `failOpen` — the operator
states the disposition explicitly. **A profile with no `geo` block behaves exactly as it does
today** — this is a strict extension, not a behavior change for existing profiles.

L4 supports **country matching only** — no continent, city, or subdivision. At the connection
level there are no request attributes to compose against, so fine-grained geo rules there would
be both hard to operate and easy to over-block; city-level decisions belong at the HTTP layer. A
profile made up of only a `geo` block, with no IP `allow`/`deny` entries, is a valid
configuration — a geo-only admission profile.

### Evaluation order

Geo is a second stage evaluated after the existing IP-based deny/allow decision, not merged into
the IP matcher itself:

| # | Condition | Result |
|---|---|---|
| 1 | IP deny hit | **Deny** (terminal) |
| 2 | IP allow hit | **Allow** (terminal — exempts geo entirely) |
| 3a | No database loaded (`Unavailable`) | `failOpen: true` → no opinion, continue to 4; `false` → **Deny** |
| 3b | `denyCountries` hit | **Deny** |
| 3c | `allowCountries` configured | hit → **Allow** (terminal); miss → **Deny** (terminal) |
| 3d | Only `denyCountries` configured, no hit | no opinion, continue to 4 |
| 4 | `defaultAction`, but **only if the profile is not in whitelist mode** | in whitelist mode: **`DenyByWhitelistMiss`**; otherwise: `defaultAction` |

Two of these rows are load-bearing and are easy to get backwards:

- **An IP allowlist hit (row 2) exempts a client from geo entirely.** With both an `allow` list
  and a `geo` block configured, a known-good IP source is never subjected to the country check —
  this exemption is the actual expressive value of merging geo into this plugin instead of a
  separate one.
- **A geo no-opinion never resurrects `defaultAction` in whitelist mode (row 4).** Since
  `defaultAction` defaults to `Allow`, if a geo no-opinion fell through to it in a whitelist
  profile, adding `denyCountries: ["RU"]` to a working IP whitelist — intended to block *more*
  traffic — would instead silently **admit every address outside the IP allowlist**, because the
  whitelist-miss path never gets reached. The implementation is specifically guarded against
  this; a whitelist profile plus `denyCountries` still denies an out-of-allowlist address exactly
  as before geo was added.
- **An `allowCountries` hit is terminal (row 3c)** — it never falls through to `defaultAction`.
  In a `defaultAction: Deny` profile (the conservative baseline this plugin's security invariant
  requires), letting an `allowCountries` hit fall through would mean `allowCountries` admits
  nobody.

### PROXY protocol and what stage-1 geo sees

`GlobalConnectionIpRestriction` runs at up to three points in a connection's lifetime, and its
notion of "client IP" — and therefore geo — differs by which one:

| Stage | Source of the address | PROXY-protocol resolved? |
|---|---|---|
| Stage 1 (`ConnectionFilter`, at `accept()`) | The raw transport peer | **No** |
| TCPRoute | The resolved client address | Yes |
| TLSRoute | The resolved client address | Yes |

PROXY protocol parsing happens strictly *after* `accept()`, so the stage-1 connection filter
cannot see the PROXY-declared client address — it sees the load balancer's own address. On a
listener with PROXY protocol enabled, stage-1 IP and geo rules therefore match the load
balancer's IP, while the TCPRoute/TLSRoute stages match the actual client's. **This is a
pre-existing, accepted property of the plugin** — its IP allow/deny rules already behave this
way today — and geo deliberately inherits the same semantics rather than diverging with a
special case; keeping the IP and geo halves of one profile consistent is worth more than a
per-mechanism carve-out. There is no `ipSource` option for this plugin to work around it.

### L4 geo denials are never logged, and carry no client address

An L4 geo denial produces **no log entry at any stage**, and records **no client address
anywhere**. The only signal is the counter `edgion_geoip_denied_total{layer,reason}` (see
[Metrics](#metrics)). This is deliberate, not an accident of the stage-1 limitation: pingora
drops a stage-1-denied connection inside its accept loop, so a log entry genuinely isn't
possible there — but the *same* no-logging rule is applied uniformly to the TCPRoute and TLSRoute
stages too, even though those stages could technically log. Uniform behavior across all three
stages is judged worth more than stage-specific detail, and a per-connection denial log is also
an amplification vector under a connection flood. HTTP-layer geo denials are unaffected by this
— they keep their normal `EdgionStatus` entry and access-log record, since a per-request log
already exists there regardless.

### No readiness gating; the operator owns the cold-start window

A fresh rollout with no database yet (typical on an `emptyDir` volume) does **not** hold back the
readiness probe. `failOpen` (per L4 `geo` block, and per HTTP `RequestRestriction`/
`GlobalAccessControl` plugin) is how you declare what should happen instead: `true` accepts a
window of unenforced geo policy, `false`
accepts a window of rejected traffic. The pre-first-load backoff (see below) bounds that window
to seconds, not the full `retry_interval_sec`, and `edgion_geoip_db_loaded` is the metric to
alert on for "is this still going on."

## Access log

There is no global switch: geo is entirely plugin-driven, and the access log only ever reports a
slot that some plugin or L4 predicate actually queried for the current request. When a route has
at least one `ClientGeo` and/or `DirectGeo` predicate, a matching request's access log entry gains
a `geo` object with the slot(s) that were queried:

```json
"geo": {
  "client": { "country": "CN", "continent": "AS", "city": "Beijing" },
  "direct": { "country": "US" }
}
```

Each slot (`client` = `ClientGeo`, `direct` = `DirectGeo`) is an independent, per-request
`OnceLock`. The access log reads whichever slots a plugin already resolved earlier in the
request — it never triggers a lookup of its own. A route with no geo rule at all therefore costs
**zero** MMDB lookups, and a route with only a `ClientGeo` rule reports only `geo.client`, never
`geo.direct`. If you want both slots in every log line, configure a predicate that reads each
one (an `onMissing: Skip` rule is enough to make the read happen without changing the request
outcome).

### Coordinates

There is no switch for latitude/longitude: a `geo` slot emits `coord` whenever the resolved
record actually carries coordinates, and omits it otherwise — the same "reads whatever a plugin
already resolved" rule as the rest of the `geo` object, with no separate gate on top. When
present, coordinates are emitted as a single `coord` string and pass through the same masking
treatment as `client-addr`/`remote-addr`.

**There is no per-key unmasking for `coord`.** Masking is controlled entirely by the
process-global sensitive-log mode — an all-or-nothing switch. The per-`LogExtSource` allowlist
mechanism that lets some ext-log fields opt out of masking does not apply here; `client-addr` and
`remote-addr` aren't individually unmaskable today either, and `coord` follows the same rule.

## Metrics

| Metric | Type | Labels | Meaning |
|---|---|---|---|
| `edgion_geoip_db_loaded` | Gauge | none | `1` when a GeoIP database is loaded, `0` otherwise. Alert on `== 0` while geo rules are configured — this is the primary "security control isn't running" signal. |
| `edgion_geoip_db_build_epoch_seconds` | Gauge | none | Build timestamp of the loaded database. Alert on staleness. |
| `edgion_geoip_update_failed_total` | Counter | `reason` (`fetch` \| `encoding` \| `size_cap` \| `stall` \| `validation` \| `rename` \| `disk` \| `deadline`) | Background *update-cycle* failures, by class. `disk` is the free-space precheck refusing to start (no request was ever issued); `deadline` is the whole cycle exceeding `download_deadline_sec` despite steady progress, distinct from `stall` (no progress within `stall_timeout_sec`) — the two call for opposite fixes. |
| `edgion_geoip_lookup_panic_total` | Counter | none | A per-request MMDB lookup panicked (caught) and degraded to `NotFound`. Distinct from `edgion_geoip_update_failed_total` above: this is a per-request event on the query path, not an update-cycle failure. |
| `edgion_geoip_denied_total` | Counter | `layer` (`stage1` \| `tcp` \| `tls`), `reason` (`deny_country` \| `allow_miss` \| `db_unavailable`) | L4 connections denied by a geo rule. Never carries a client address; HTTP denials are deliberately absent here — they already appear in the access log. |

## Multi-replica update behavior, jitter, and the `.meta` validator

The updater's request cycle: a conditional `GET` (`If-None-Match` / `If-Modified-Since`); a
`304` sleeps until the next interval without touching disk; a `200` with a non-`identity`
`Content-Encoding` is rejected loudly (the updater does not decompress — point `update_url` at a
plain MMDB, not a `.mmdb.gz`); a `200` with `identity` encoding streams to `<db_path>.tmp` under
the `max_db_bytes`/`stall_timeout_sec`/`download_deadline_sec` bounds; on success, `rename` →
re-mmap → swap → persist the validator to `<db_path>.meta`.

**Backoff has two regimes.** Before the *first* successful load, retries use a short escalating
backoff (1s, doubling, capped at 30s) — `retry_interval_sec`'s default of 600s is tuned for
steady state and would otherwise leave a cold-started pod effectively unprotected for ten
minutes. After the first success, ordinary failures use `retry_interval_sec` plus jitter.

**Jitter applies to the very first fetch too, not only to retries.** Without it, a 200-pod
rolling restart issues 200 simultaneous unconditional GETs against the mirror. Jitter alone
doesn't make those conditional, though — that's the job of the `.meta` file below.

**`<db_path>.meta` persists the ETag/Last-Modified validator** used to make requests conditional
in the first place. On an `emptyDir` volume, `.meta` is lost on every pod restart exactly like
the database itself — but if `.meta` is missing while the database file *does* still exist (a
plausible partial-persistence scenario, e.g. a PVC that only recently started being used, or a
manual recovery), the updater seeds `If-Modified-Since` from the database file's mtime instead
of skipping straight to an unconditional fetch. The practical consequence to plan for: losing
`.meta` — whether from an `emptyDir` restart or any other cause — turns the *next* update cycle
into a full, unconditional download rather than a cheap `304`, even if the remote content hasn't
actually changed.

## Troubleshooting

| Symptom | Likely cause | Fix |
|---|---|---|
| `edgion_geoip_db_loaded` stays `0` even though your YAML clearly sets `geoip.db_path` | `geoip:` is at the document root instead of nested under `gateway:` | Move it under `gateway:` — see [the top-level nesting trap](#-the-single-most-common-way-to-get-this-wrong) |
| Config load fails with an unknown-field error mentioning a `geoip` sub-key | A camelCase typo inside the `geoip` block (e.g. `dbPath`) | Use snake_case — `db_path`, `update_url`, etc. |
| `edgion_geoip_db_loaded` stays `0` after a fresh deploy | Normal at first start — no bundled database, and the updater hasn't completed its first cycle yet | Wait for the first successful cycle, or check `edgion_geoip_update_failed_total{reason}` for a stuck failure |
| Gateway process dies with no panic log, mid-request, shortly after a database file was touched externally | Something wrote `db_path` in place instead of rename-swapping it — SIGBUS | Stop whatever wrote in place; only the built-in updater or a same-filesystem temp-file-plus-rename writer are supported |
| Updater cycles fail permanently with `reason="disk"` (or an `ENOSPC`-flavored error) | Update volume undersized for a swap (needs `max_db_bytes` for the `.tmp` + 2× the current database size for the old inode/rename double-hold) | Resize the volume per the formula above, not to 3× `max_db_bytes` |
| A `country` + `city` rule group never matches anything, in Blacklist mode | The loaded database's records don't carry city data for this rule's `onMissing: NoMatch` predicate — see [no "database tier" concept](#there-is-no-database-tier-concept--a-missing-dimension-is-ordinary-absence) | Check the access log's `geo` block against your provider's documentation for what the loaded database actually resolves; set `onMissing` deliberately, or drop the `city`/`subdivision` predicate |
| Every request is denied with 403 during a cold start, on a Whitelist-mode profile with only geo rules | All-Skip whitelist hazard: `onMissing: Skip` on every rule in the group, plus a plugin-level `failOpen: true`, evaluated before the database has ever loaded | Use `failOpen: false` for a diagnosable rejection instead, or ensure at least one non-geo predicate is present in the group |
| A `city` rule with a translated-language value never matches | `city` only ever compares against `names["en"]` | Use the English city name |
| A blacklist-mode `ClientGeo` rule is trivially bypassed by any client | `RealIp`'s `trustedIps` is scoped too broadly (e.g. `0.0.0.0/0`) — `ClientGeo` degrades to attacker-controlled input | Scope `trustedIps` to your actual proxy egress ranges, or switch the rule to `DirectGeo` |
| A `GlobalAccessControl` group decides inconsistently between its IP precondition and its geo rule | `ipSource: ClientIp` paired with `DirectGeo` (or the reverse) — GAC's `ipSource` naming is inverted relative to `RequestInfo` | Pair `ipSource: ClientIp` with `ClientGeo`, and `ipSource: RemoteAddr` with `DirectGeo` — the gateway also emits an ingest WARN for this specific mismatch |
| An L4 geo deny happened but there's no log line anywhere for it | Expected — L4 geo denials are never logged at any stage, by design | Use `edgion_geoip_denied_total{layer,reason}` as the only signal |
| A whitelist L4 profile with an IP allowlist plus `denyCountries` seems to admit traffic it shouldn't | Check whether the admitted client hit the IP `allow` list — an IP allow hit is terminal and exempts geo entirely (row 2 of the evaluation order) | Working as designed; narrow the `allow` CIDRs if the exemption is too broad |

## Related documents

- [Gateway Configuration](../configuration/gateway-config.md) — full `GatewayConfig` schema reference, including `geoip`
- [Real IP](../../user-guide/edgion-plugins/real-ip.md) — `trustedIps` configuration that governs whether `ClientGeo` is trustworthy
- [Metrics](../observability/metrics-inventory.md) — full metrics inventory, including the four `edgion_geoip_*` series
