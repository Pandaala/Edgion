---
name: service-cb-state-new-ctor
description: Current identity-bound Service outlier ownership and preserved regression boundary.
knowledge_type: regression-guards
---

# Canonical Service outlier state construction

`ServiceCbState::new(config, policy_identity) -> Arc<Self>` is the one
construction site for normalized configuration, captured policy identity,
identity-keyed passive states and service arbitration mutex. No socket-hash
execution map remains (ARCH-018). Config publication invokes this constructor
only for a new or changed policy; identical config preserves the current Arc.

The historical backends-F010 correction removed duplicated struct literals in
Occupied/Vacant entry branches. Those branches are superseded, while the useful
single-constructor boundary remains. Re-evaluate if additional construction sites
or another execution map are introduced. Source: outlier_detection/store.rs;
coverage: config replacement/no-op store tests. No performance measurement is
claimed by the structural removal.

Provenance: backends-F010 (2026-05-24); ARCH-018.
