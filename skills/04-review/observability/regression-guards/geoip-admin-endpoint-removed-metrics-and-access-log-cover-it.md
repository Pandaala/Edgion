---
name: geoip-admin-endpoint-removed-metrics-and-access-log-cover-it
description: Use when reviewing a proposal to add (or re-add) a dedicated admin-introspection endpoint for a subsystem that already has metrics and/or an access-log field; covers why `GET /geoip` was removed once its fields lost their original justification.
status: fixed
finding_id: review-geoip-consolidation-02
closed: 2026-08-17
knowledge_type: regression-guards
---

# `GET /geoip` admin endpoint removed — metrics and the access log already cover it — FIXED

## Conclusion

The gateway admin API's `GET /geoip` endpoint, `GeoIpAdminStatus`, `admin_status()`,
and the process-wide `ConfigSnapshot`/`CONFIG_SNAPSHOT` that backed it are
removed. Every field the endpoint reported had lost its original justification
by the time of this review: `build_epoch` and `loaded` duplicate the
`edgion_geoip_db_build_epoch_seconds` / `edgion_geoip_db_loaded` metrics;
`database_type` was the sole consumer of the "database tier" heuristic that a
separate, earlier finding already removed as unreliable; `enabled` duplicated
`GeoIpConfig` state removed by the sibling finding in the same review batch;
and `last_update`/`last_failure_reason` were never implemented. Operators
diagnose "why is my geo rule inert" from `edgion_geoip_db_loaded` (is a
database loaded at all) plus the access log's `geo` block (what a given
request's lookup actually resolved to), not from a process-wide admin
snapshot.

Fix suggestions of the form "keep the endpoint but trim it to just `loaded`"
or "keep it as a stable diagnostic surface even with no distinct fields left"
are **not accepted** — see rationale below.

## Core Rationale

**1. An endpoint whose every field is now redundant is a maintenance cost with
no offsetting value.**

`db_path`/`loaded`/`build_epoch` are exactly `edgion_geoip_db_loaded` and
`edgion_geoip_db_build_epoch_seconds` reshaped into JSON. `node_count` had no
operator-facing use case documented anywhere. Once `database_type`'s only
consumer (the tier-mismatch warning) was removed, nothing new was added to
replace it — the endpoint quietly became a redundant re-projection of data
already exposed through the standard metrics path, not a distinct
introspection capability.

**2. A redundant endpoint is still an attack-surface and secret-hygiene cost.**

Every additional bearer-token-gated admin route is one more thing to keep out
of `update_url` (secret-bearing) accidentally, one more route to check for the
CRD schema / redaction invariants documented in the project's top-level
constraints, and one more thing a future field addition might carelessly leak
through. A route that provides zero information not already available through
the metrics/access-log path is pure downside once its original justification
is gone.

**3. Metrics + access log is the project's established diagnostic pattern for
this subsystem, not a downgrade.**

Both replacement signals already existed before this fix: `edgion_geoip_db_loaded`
answers "is anything loaded at all" (the gauge an operator alerts on), and the
access log's `geo` block (see the sibling finding on lazy per-slot resolution)
answers "what did a specific request actually resolve to" per-request, which
is strictly more precise than a process-wide snapshot for diagnosing "why is
my rule inert for this client". No new metric or log field was added to make
this work — the existing signals were already sufficient once the truly
redundant fields were named as such.

## Fix Suggestions Not Accepted

- Trim the endpoint to just `loaded` (or `loaded` + `db_path`) instead of
  removing it — still fully redundant with `edgion_geoip_db_loaded`, and
  keeping *any* endpoint keeps the maintenance/attack-surface cost of item 2
  above for zero net new information.
- Keep the endpoint as a "stable place to add fields later" — YAGNI; add the
  endpoint back if and when a field is actually needed that metrics/access-log
  genuinely cannot express, with that field's justification stated at
  introduction time.
- Move `database_type` reporting into a metric label — `database_type` is
  operator-supplied free text (a MaxMind product name); putting it on a metric
  label risks unbounded cardinality if operators rotate database sources, and
  no alerting/dashboarding use case for it was identified once the tier
  heuristic was removed.

## Re-evaluation Triggers

Re-open only if:

- A genuinely new, distinct-from-metrics diagnostic need appears (e.g. the
  updater starts tracking `last_failure_reason` in a way that cannot be
  expressed as a bounded-label counter) — introduce the endpoint fresh, scoped
  to that specific new field, not as a revival of the old shape.
- Operators report that `edgion_geoip_db_loaded` plus the access log's `geo`
  block is insufficient to diagnose a real incident — capture the concrete gap
  before adding a new surface.

## Reference Cases

- Source finding: `tasks/review-geoip-consolidation/02-low-remove-geoip-admin-endpoint.md`.
- Sibling finding (same consolidation batch, global switch + eager access log removal):
  `observability/geoip-no-global-enabled-switch-access-log-lazy.md`.
- Design doc: `docs/superpowers/specs/2026-08-14-geoip-country-restriction-design.md` §4.5.
- Code (removed): `edgion-gateway/src/geoip/mod.rs` (`GeoIpAdminStatus`, `admin_status`,
  `ConfigSnapshot`), `edgion-gateway/src/api/mod.rs` (`geoip_status`, `/geoip` route).

## Resolution references

- `review-geoip-consolidation-02` — 2026-08-17.
