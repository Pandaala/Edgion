---
id: ARCH-063
status: confirmed
kind: architecture-debt
priority: P3
partition: PART-206
analyst: /root/verify_ip_union
baseline: 22f0fb8b6f4353f689ca4ddc172a4dfd98a2d2929dd1709240cbe4a1b3796471
---

# Retire the orphan ConfigData overlay pipeline while keeping the live borrowed owner

## Current ownership and evidence

edgion-resources/src/resources/edgion_config_data/overlay.rs exports
apply_override and OverlaySignal. mod.rs reexports them and also exposes
effective_switch; region_route.rs:61 exposes merge_region_route_override.
A workspace-wide Rust search, including hidden source directories and excluding
build/Git data, finds exactly five apply_override calls, all in overlay_test.rs.
They use ToyBase/ToyOverride/ToyEntry rather than ConfigEntry. The other three
API pieces have no Rust callers outside their definitions, reexports and those
fixtures or an ignored documentation example.

The live RegionRoute implementation uses a different owner:
EffectiveRegions holds a borrowed base slice and an optional Arc<EdgionConfigData>.
compute_effective_regions loads the paired raw resource/table proof, checks the
envelope, variant and consumer modulo, and keeps a valid immutable overlay
generation alive. Deref borrows its whole table; no owned merge helper runs.
The Gateway store derives RegionTableValidation at publication and publishes it
atomically with the raw resource.

The Controller's retained effective-view API performs a validated owned JSON
projection in apply_region_override. It also does not use either overlay helper.
Its management endpoint and failover write path are live requirements and are
outside this removal proposal.

## Concrete maintenance cost

The orphan API presents a production overlay pipeline whose tests exercise only
extract/merge closures. It owns no envelope policy, table proof, modulo check or
live-generation lifetime. overlay.rs nevertheless describes RegionRouteOverride
as its remaining production user, and RegionRoute's module documentation says
the real consumer applies apply_override. Those claims are false for the current
implementation.

A change to RegionRoute overlay availability or validation therefore exposes a
public generic entry point, an owned table-merging entry point, a switch helper
and five detached fixture assertions that appear to describe the serving path
but never run it. Reviewers must distinguish these from the actual borrowed
owner and its tests. This is an avoidable source/test ownership burden, not an
observed traffic defect or measured runtime cost.

The generic helper is not an alternative canonical table validator. In
particular, its three-value signal is not the current consumer's complete
missing/disabled/type-mismatch/invalid policy. The abstract whole-replace model
remains useful without retaining an unused callable implementation.

## Bounded improvement and alternative

After a fresh caller audit, retire overlay.rs and its module/export, the unused
merge_region_route_override function/export, and the unused effective_switch
helper with its ignored pipeline example. Remove concrete-call documentation
that names that retired pipeline; describe the conceptual base/overlay model
and the actual borrowed owner in the existing ConfigData architecture page and
RegionRoute module documentation. Keep the current schemas and real runtime
selection code.

Do not route RegionRoute through the owned merge to manufacture a consumer.
With the current return type, selecting an overlay clones the full table, and
supplying the long-lived base by value would require a base copy or another
adapter. No such copy runs today; this is a rejected migration, not a finding of
current request-path allocation. A borrowed generic replacement would add
another abstraction with no second real consumer and would still need the
feature's validation and generation owner.

Retire fixture-only assertions only with an explicit coverage mapping. Keep
unit coverage of the actual absent and wrong-type fallback, whole-table
replacement and None inheritance. Existing RegionRoute unit tests cover
base/missing/disabled/invalid outcomes, old-generation lifetime and consumer
modulo; resource tests cover None versus an invalid empty replacement; store
tests cover paired proof replacement/invalidation. Where those tests do not
directly prove wrong-type diagnostics or complete replacement of a multi-entry
base, add focused assertions through the current owner. Toy x/y field merging
and all-fields-at-once have no production counterpart after the unused generic
API is retired; record that reason rather than claiming equivalent real fields.
Do not reduce canonical validator, wire, security or historical regression tests.

## Invariants, original decisions and future verification

Preserve typed RegionRouteOverride data, whole replacement, safe authored base,
envelope-only enable semantics and currently inert variant enable/active.
Preserve None inheritance, invalid empty replacement, strict shared table
validation, per-consumer modulo, one-hop failover, raw/proof generation pairing
and held Arc lifetime. Keep actual owning reference permission/identity checks,
hot updates, access-log/PluginLog and existing bounded metric signals. No schema,
reference policy, Controller workqueue, endpoint, WAF/IP/Selector policy or
observability expansion is required.

The original ConfigData page explicitly labels the apply signature conceptual.
The RegionRoute effective-table and ConfigData atomic-publication guards require
the current borrowed table/proof mechanism. The retained-management-API decision
requires keeping the Controller surface. None requires retaining these unused
Rust helpers. Pre-release deletion requires no compatibility wrapper; the
unit-testing guide still requires preserving meaningful algorithm and boundary
coverage. This is separate from ARCH-028's active matcher-union allocation and
ARCH-031's orphan OIDC JSON reader.

Future checks should confirm zero remaining exports/calls or broken documentation
links, preserve the actual unit-test contracts above and validate compilation,
format and documentation consistency. Historical integration acceptance is not
fresh execution evidence. No builds, tests or network operations were executed
during this analysis; external repository consumers were not audited.
