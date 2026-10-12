---
name: geoip-no-global-enabled-switch-access-log-lazy
description: Use when reviewing findings about a redundant global enable switch duplicating plugin-level configuration, or about an access-log field forcing a resolution/lookup just to keep the field always present; covers the GeoIP consolidation precedent (removed `GeoIpConfig.enabled`, made the `geo` access-log block per-slot-lazy).
status: fixed
finding_id: review-geoip-consolidation-01
closed: 2026-08-17
knowledge_type: regression-guards
---

# GeoIP has no global `enabled` switch; the access log never forces a lookup — FIXED

## Conclusion

`GeoIpConfig.enabled` is removed. Geo capability is driven entirely by whether a
plugin or L4 predicate actually queries the `remote_geo()`/`direct_peer_geo()` slot for a
given request; `db_path` unset means no database (`Unavailable`), and
`update_url` unset means no updater task — no separate switch is needed. The
access log's `geo` block (`edgion-gateway/src/observe/access_log/entry.rs`)
reads already-resolved slots through `RequestInfo::resolved_geo()`, which uses
`OnceLock::get()` without allocating optional geo state, not the
`get_or_init` accessor plugins use, so it never queries the database on its
own — a route with no geo rule at all costs zero MMDB lookups. `GeoLogBlock`
reports `Option<GeoSlotLog<'a>>` per slot (`remote`/`direct_peer` independently), and
`GeoLogBlock::from_slots` returns `None` (the whole `geo` key omitted) only
when neither slot was resolved by anything.

Fix suggestions of the form "keep a coarse enable/disable flag as a fast-path
skip" or "resolve both slots eagerly so the access log schema is always
uniform" are **not accepted** — see rationale below.

## Core Rationale

**1. A global switch duplicates information the plugin config already carries.**

`GeoIpConfig.enabled` answered a question the loaded config already answers
more precisely: `db_path` set = "there is something to load"; a predicate using
`remote_geo()`/`direct_peer_geo()` on a route/profile = "this specific traffic
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
downstream of whatever a plugin already resolved:
`ctx.request_info.resolved_geo()`, never the accessor that would populate them.

**3. A schema that appears only when queried is not "unstable" in a harmful
sense.**

The original design worried that an unstable `geo` key shape would confuse
downstream indexing. The resolved shape is `Option<GeoLogBlock<'a>>` with
`skip_serializing_if` at three levels (the whole block, and each of
`remote`/`direct_peer` independently) — this is the same pattern already used for
`genai`, `match_info`, and other optional access-log sections. An operator who
wants `geo` in every log line gets it by configuring a predicate that
already reads the GeoIP-backed slot (`remote_geo()` / `direct_peer_geo()`) — typically
a `geoip_location` rule, or any other Condition that queries that slot as
part of a real enforcement decision. There is no Skip outcome and no
dedicated force-lookup-without-verdict knob; the log field and the
enforcement decision share the same underlying lookup, so there is nothing
left to keep in sync between "did I turn geo on" and "did I write a geo
rule".

## Borrowed projection ownership (ARCH-019)

RequestInfo owns the lazy slots and their immutable `GeoLookup::Found(Arc<GeoInfo>)`
facts. `GeoSlotLog` borrows country, continent, subdivision and city strings from
those facts through `as_deref()`; its lifetime flows through `GeoLogBlock` into
the context-borrowing `AccessLogEntry`. This removes the four dimension clones
per found slot without another lookup or enum match. The computed coordinate
string remains owned and uses the unchanged canonical `masked_opt` serializer.

Terminal, connection-established and debug consumers serialize while the context
is alive. `to_json()` returns an independently owned string for sink/header use.
Provider reload replaces the database generation, not facts already captured in
request slots. Keeping owned dimension copies solely for temporary test fixtures
is unnecessary; fixtures bind their lookup owners instead.

Regression coverage lives in `tests/unit/observe/access_log/entry_test.rs`: both
context slots retain dimension pointer/length identity and JSON escaping; partial
dimensions and coordinates omit missing fields; unresolved optional state stays
unallocated; captured facts survive local provider replacement and serialized
JSON survives context drop. Existing status and terminal-only WAF tests remain.
These describe coverage, not a fresh passing result; see the implementation task
for executed checks. Gateway coordinate tests check presence, not every sensitive
mode. The unchanged serializer wiring and resources sensitive-mode helper tests
provide scoped evidence, not an end-to-end mask-mode verification. No measured
throughput or latency gain is claimed. Re-evaluate ownership if a future consumer
needs to retain an unserialized projection after its context is dropped.

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

## Resolution references

- `review-geoip-consolidation-01` — 2026-08-17.
- `ARCH-019` — borrowed dimension projection; implementation and verification
  tracked in `tasks/pending/architecture-borrowed-geo-access-log/`.
