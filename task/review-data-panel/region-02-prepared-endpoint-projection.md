# RegionRoute prepared endpoint projection

Status: deferred for user decision. No runtime change.
Source baseline: `84a83708b`.

## Evidence and scope

RegionTableValidation::new calls RegionDef::parse_endpoint for every row during
table validation, but its compact Copy proof retains only the maximum hash bound.
For each selected nonlocal region, RegionRoute::route_to calls the same parser
again before constructing the external jump. This repeats hostname-label checks,
numeric IPv4 or bracketed IPv6 validation and port/default resolution. Local-region
pass-through returns before this parsing work.

The parser already returns a borrowed host; it does not allocate a host buffer.
The later domain String and request-derived SNI belong to the jump and are not
eliminated by preparing endpoint syntax. DNS resolution and dial-address policy
are separate later operations, not candidates for this cache.

Base tables are owned by the immutable plugin config. Live overlay raw resources
and validation proofs are published in one ArcSwap snapshot; the getter reads both
from that same snapshot, and EffectiveRegions retains the selected resource Arc.
A parsed projection must remain coupled to the exact row and generation, including
TLS because an omitted port resolves to 80 or 443.

## Decision requested

Choose whether to retain parsing or prepare one canonical endpoint projection:

- Store a host byte range and resolved port beside each validated row. Borrow
  through the retained raw endpoint owner; prove range boundaries and table/row
  identity without unsafe lifetime extensions or self-referential storage.
- Store an owned parsed host and port. This simplifies projection but duplicates
  endpoint text and adds retained allocation per row/generation, including rows
  never selected or used only for local pass-through.
- Keep the current allocation-free parser until measurement justifies retained
  metadata and a broader publication interface.

Base and overlay preparation need one consistent parser/representation, not two
independent endpoint grammars. The overlay proof is currently cheap to copy with
outer snapshot maps; adding vectors directly changes unrelated update costs.
Review immutable shared ownership as an alternative, not permission for a second
store or request-path lazy initialization. Include old in-flight generations in
retained-memory measurements.

This is distinct from region-01's name/range/failover selection indexes. A future
joint prepared-table design may share an owner, but either decision must be
authorized and reviewed with an exact cross-crate file allow-list.

## Acceptance before implementation

Use IMPL and independent plan/final review. Preserve original hostname spelling
and trailing dot, IPv6 bracket removal, strict numeric IPv4 syntax, port bounds,
and TLS-derived defaults. Test omitted/explicit ports, IPv4/IPv6/DNS, malformed
hosts and ports, and TLS-only generation changes. Keep current resource schema
and transport rules for derived fields; no new operator field is needed.

Cover base construction, full/incremental overlay publication, replacement and
deletion, old-generation readers and same-key updates. Preserve whole-table
replacement, None inheritance, invalid empty overlay rejection, and allowed
no-rule empty base configurations. Missing, disabled, wrong-type, invalid or
consumer-modulo-incompatible overlays still fall back to the validated base.
Retain configured row order, gaps, one-hop failover and local-region bypass.

Never pair a projection from one snapshot with another raw table. Preserve
fallback metrics, dye outcomes, reference authorization and jump rejection.
Incoming Host still supplies request-time SNI; DNS freshness, canonical resolver
use and post-resolution SSRF checks remain mandatory. No endpoint syntax proof
authorizes an address or substitutes for a dial-time check.

Measure parser CPU separately from DNS, selection and jump allocation, using
short/long DNS names, IPv4/IPv6, local-pass-through and table-size distributions.
Compare publication work, per-row retained bytes and concurrent-generation cost.
No throughput or allocation reduction is established by this source audit.

## Sources

- Gateway `plugins/http/region_route/plugin.rs`: route_to and EffectiveRegions.
- Gateway `plugins/runtime/edgion_config_data/conf_handler.rs`: paired getter,
  table validation and snapshot publication.
- Resources `resources/edgion_plugins/plugin_configs/region_route.rs`: parse_endpoint.
- Resources `resources/edgion_config_data/region_route.rs`: RegionTableValidation.
- `skills/04-review/routing/regression-guards/region-route-effective-table-validation.md`.
- `skills/04-review/ssrf/mandatory-design/upstream-address-safety.md`.

Fresh unchanged-code verification passed 23 Gateway RegionRoute tests. This
verifies the current baseline, not any proposed representation or live reload.
