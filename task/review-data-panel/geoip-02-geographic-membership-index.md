# Prepared GeoIP geographic membership

Status: deferred representation and performance decision for the user.
Source baseline: `f6dc22bb5`. No runtime or schema change.

## Evidence and scope

Connection and HTTP GeoIpLocation callbacks reach the shared Resources
`GeoRule::matches` and `dimension_matches` in
`edgion-resources/src/resources/edgion_plugins/plugin_configs/geo_rule.rs`.
For country, continent, subdivision and city, membership scans Vec<String>
with borrowed exact equality and stops at the first hit. Missing facts return
false. This path already allocates nothing; an index would target comparisons,
not eliminate a request-time allocation.

Compilation uppercases country, continent and subdivision values with ASCII
normalization. City remains unchanged and case-sensitive. Unknown country or
continent codes produce a warning but stay in the list. The inspected
GeoRule::field_error, GeoRuleGroup::check_fields and
GeoIpLocationConfig::check_fields functions impose no geographic value-count
limit; this is not a claim about every surrounding resource or transport bound.

Both Gateway implementations validate and normalize a private config clone.
The resource structs expose mutable public values and their pure matcher can
also receive directly constructed rules. Replacing the shared linear scan with
binary search would therefore require an explicit prepared-owner contract:
arbitrary input is not proven sorted. The current configuration representation
and runtime CIDR reference grid must not silently become inconsistent.

The existing [record decoding task](geoip-01-record-decoding.md) concerns MMDB
decoding and projection. This task concerns membership after facts are resolved;
it grants no permission to change provider decoding, lazy slots or fact ownership.

## Alternatives for a separate decision

- Retain linear matching, especially for the common small-list case.
- Prepare sorted membership storage or an index into owned normalized values.
- Prepare a hash-based membership representation, potentially above a measured
  list-size threshold.

Choose one canonical representation and owner for both HTTP and Stream. Evaluate
whether it can reuse existing private storage or needs extra generation-owned
memory. Do not duplicate policy semantics in the two callbacks, introduce
self-referential borrowed state, or add an unrequested serialized/runtime-cache
field to the public schema.

A fixed known-country/continent bitset alone is insufficient: unknown values
remain accepted and may match equal supplied facts. Any finite-domain shortcut
must retain exact matching outside that domain. Small lists can make hashing or
additional indirection more expensive than the existing scan. None of these
alternatives is selected or claimed faster by this task.

## Acceptance for an approved implementation

Preserve exact byte equality after the current ASCII-only normalization, city
case sensitivity, unknown code behavior, duplicate-value membership, missing
facts and direct uncompiled-rule behavior. Do not add trimming, Unicode folding,
unknown-code rejection or new admission limits as an optimization.

Keep rule order, group AND/OR short circuits, deny-before-allow precedence,
whitelist terminal decisions and defaultAction. CIDR rules still use the shared
canonical IpList owner and existing side/group/rule coordinates. Do not reorder
rules or change CIDR callbacks to support a geographic-only index.

Required-reference availability remains checked against the captured snapshot
before database failOpen handling. Invalid generations remain fail closed and
clean replacements recover. Preserve CIDR-only zero-lookup behavior, HTTP
parse-only mode, Stream parse-only rejection, source selection and the distinct
Unavailable/Failed/NotFound outcomes. Access logging must not trigger a lookup;
keep existing bounded denial metrics and log ownership.

Compare small, large, duplicate-heavy, early-hit, late-hit and miss lists with
short codes and long city/subdivision values. Measure request/connection CPU,
cold construction cost, total retained capacity and overlapping old/new runtime
generations. Include normalization, warnings and publication behavior in parity
tests. Any future implementation needs its own frozen file scope and IMPL review.

## Original knowledge and verification limits

- [Invalid Stream generations and CIDR reference boundaries](../../skills/04-review/plugin-runtime/regression-guards/invalid-stream-geoip-generation-fail-closed.md)
- [Lookup failures and failOpen](../../skills/04-review/observability/regression-guards/geoip-lookup-failures-follow-fail-open.md)
- [Lazy lookup and access-log ownership](../../skills/04-review/observability/regression-guards/geoip-no-global-enabled-switch-access-log-lazy.md)

This source audit establishes the current linear membership path and its
constraints. Current unit tests do not validate a proposed index. No matching
benchmark or retained-generation memory measurement ran for this task.
