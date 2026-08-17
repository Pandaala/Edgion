---
name: geoip-no-global-enabled-switch-access-log-lazy
description: Use when reviewing findings about a redundant global enable switch duplicating plugin-level configuration, or about an access-log field forcing a resolution/lookup just to keep the field always present; covers the GeoIP consolidation precedent (removed `GeoIpConfig.enabled`, made the `geo` access-log block per-slot-lazy).
status: fixed
finding_id: review-geoip-consolidation-01
closed: 2026-08-17
---

# GeoIP has no global `enabled` switch; the access log never forces a lookup — FIXED

## Conclusion

`GeoIpConfig.enabled` is removed. Geo capability is driven entirely by whether a
plugin or L4 predicate actually queries the `ClientGeo`/`DirectGeo` slot for a
given request; `db_path` unset means no database (`Unavailable`), and
`update_url` unset means no updater task — no separate switch is needed. The
access log's `geo` block (`edgion-gateway/src/observe/access_log/entry.rs`)
reads each per-request slot with `OnceLock::get()` (non-forcing), not the
`get_or_init` accessor plugins use, so it never queries the database on its
own — a route with no geo rule at all costs zero MMDB lookups. `GeoLogBlock`
reports `Option<GeoSlotLog>` per slot (`client`/`direct` independently), and
`GeoLogBlock::from_slots` returns `None` (the whole `geo` key omitted) only
when neither slot was resolved by anything.

Fix suggestions of the form "keep a coarse enable/disable flag as a fast-path
skip" or "resolve both slots eagerly so the access log schema is always
uniform" are **not accepted** — see rationale below.

## Core Rationale

**1. A global switch duplicates information the plugin config already carries.**

`GeoIpConfig.enabled` answered a question the loaded config already answers
more precisely: `db_path` set = "there is something to load"; a `ClientGeo`/
`DirectGeo` predicate configured on a route/profile = "this specific traffic
actually needs a lookup". A third, coarser knob on top adds an operator-facing
concept ("did I remember to also set `enabled: true`?") with no independent
information content, and — per the original finding — silently doubled the
per-request MMDB query count once turned on, because the access log used
`get_or_init` to guarantee a `geo` object on every line.

**2. Forcing a lookup for logging purposes fights the lazy-slot design's own
invariant.**

The per-request slots are `OnceLock<GeoLookup>` specifically so that "no geo
rule on this route" means "zero MMDB queries" for that route. An access-log
path that calls the `get_or_init` accessor defeats that invariant for every
request once the (now-removed) switch was on, regardless of whether any rule
on the route actually needed the value. The fix keeps the log strictly
downstream of whatever a plugin already resolved: `ctx.client_geo.get()` /
`ctx.direct_geo.get()`, never the accessor that would populate them.

**3. A schema that appears only when queried is not "unstable" in a harmful
sense.**

The original design worried that an unstable `geo` key shape would confuse
downstream indexing. The resolved shape is `Option<GeoLogBlock>` with
`skip_serializing_if` at three levels (the whole block, and each of
`client`/`direct` independently) — this is the same pattern already used for
`genai`, `match_info`, and other optional access-log sections. An operator who
wants `geo` in every log line gets it by configuring a predicate that reads
each slot (an `onMissing: Skip` rule is enough to force the read without
changing the request outcome) — the log field and the enforcement decision
share the same underlying lookup, so there is nothing left to keep in sync
between "did I turn geo on" and "did I write a geo rule".

## Fix Suggestions Not Accepted

- Re-add `enabled` as a fast-path short-circuit ("skip the `init` work when
  `false`") — `db_path`/`update_url` being unset already short-circuits `init`
  with no separate flag needed.
- Keep `get_or_init` in the access log path and gate it behind the (removed)
  switch — this is the exact eager-resolution cost the finding identified;
  moving the trigger elsewhere does not remove it.
- Emit an always-present `geo: {}` object (empty instead of absent) for schema
  uniformity — this still requires deciding *something* triggers the object's
  presence, and an empty object gives dashboards a worse signal than an absent
  key (`skip_serializing_if` is the existing idiom for "this feature did not
  apply to this request").

## Re-evaluation Triggers

Re-open only if:

- A future feature needs a process-wide "geo subsystem on/off" concept that is
  genuinely distinct from "is a database configured" (e.g. a licensing gate) —
  in which case the new switch must NOT be read by the access log path, only
  by `init`.
- The access log is asked to guarantee `geo` presence on every request
  regardless of plugin configuration — that is a product decision to make
  geo mandatory infrastructure, not an observability bug fix, and needs its
  own design review given the MMDB-query cost this finding removed.

## Reference Cases

- Source finding: `tasks/review-geoip-consolidation/01-medium-drop-global-enabled-and-eager-access-log.md`.
- Sibling finding (same consolidation batch, admin endpoint removal):
  `observability/geoip-admin-endpoint-removed-metrics-and-access-log-cover-it.md`.
- Design doc: `docs/superpowers/specs/2026-08-14-geoip-country-restriction-design.md` §2, §5.5.
- Code: `edgion-gateway/src/geoip/config.rs` (`GeoIpConfig`), `edgion-gateway/src/geoip/mod.rs` (`init`),
  `edgion-gateway/src/observe/access_log/entry.rs` (`GeoLogBlock`, `GeoSlotLog`).
