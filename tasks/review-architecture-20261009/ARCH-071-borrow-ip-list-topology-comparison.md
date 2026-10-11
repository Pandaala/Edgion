---
id: ARCH-071
status: candidate
kind: architecture-debt
priority: P3
partition: PART-217
analyst: /root
baseline: 22f0fb8b6f4353f689ca4ddc172a4dfd98a2d2929dd1709240cbe4a1b3796471
---

# Borrow ConfigData fields when checking prepared IpList topology

Both canonical owning-reference catalogues retain necessary cold snapshots but clone an actual reference solely to compare it. In condition_fields.rs, PreparedIp::matches checks each stored FieldWire against ip_wire(wire), whose only work is cloning name and optional namespace into a temporary FieldWire::ConfigData. In ip_list_fields.rs, PreparedIpListReferences::check compares wire(r) == s.wire using the same owned projection. Those temporaries are immediately discarded. Snapshot construction legitimately needs owned reference strings; equality against an actual owner loan does not.

The comparison runs in real cold publication and Controller reconciliation. Gateway PluginRuntime::add_request_entries prepares and binds actual HTTP IpRestriction owners, and StreamPluginGeneration::compile prepares and binds actual connection/TLS/GCIR owners. Controller ip_list_refs constructs and binds the standalone catalogue for dependency collection, marking and readonly status. Its condition_tree expansion, credential pass and readonly status visit PreparedConditionReferences; the IP leaf reloan also checks PreparedIp::matches before issuing tokens. No request-throughput regression or measured heap result is claimed. For any successful full comparison, the source copies the sum of authored name bytes plus present namespace bytes, in addition to the stored snapshot's required strings.

Compare the stored variant by borrowed pattern matching instead: require FieldWire::ConfigData and compare its name and Option<String> namespace directly with the actual EdgionConfigDataRef fields. Keep ip_wire/wire only for preparation sites that create an owned snapshot, or rename them to make that ownership purpose explicit. The two small borrowed checks need no public projection enum, generic registry or new reference framework; a private canonical helper is optional only if it makes both sites simpler without broadening authority.

Preserve every existing rebind gate: metadata identity/UID/generation, stage presence and lengths, original ordinals/variant/enable, deterministic profile key order, ordered reference length, allow/deny side and exact optional namespace spelling. An omitted namespace must remain different from explicit empty in the snapshot even though later target resolution has its own default. Ignore ref_denied and resolved_ips exactly as today: Controller-owned material/denial updates must not invalidate authored topology. A wrong stored FieldWire variant must return false. Existing standalone owner failures remain distinct from invalid target slots; inline-only constructors retain their current treatment. Condition activity and RAP partial-good/warning behavior are separate from credential supported-stage gating and remain unchanged.

Keep the actual immutable/mutable owner loan and its lifetime restrictions, synchronous condition tokens that cannot freeze, private role/slot creation, validated identity before lookup/Grant/dependency, register-before-read lifecycle, canonical matcher/snapshot publication and one ConfigData snapshot per decision. No extra request allocations, logs, locks, material copies or separate cache are needed. Borrowed equality removes only transient comparison strings; catalogue vectors, sorted keys and snapshot ownership are not claimed to become allocation-free.

Original condition-set-single-owner authority, current standalone IpList publication guard and the active resource-reference-contract specification require source/topology checks, not their owned comparison projection. Relevant actual tests were fully read: condition_fields_test covers source/activity/container/reference drift, material mutation, invalid IP identities and warning recheck; ip_list_fields_test covers disabled owners, source versus target errors, marker writeback, profile drift and side/order exchanges. Future verification should retain these bodies and cover name/namespace/presence/ordering changes plus wrong-variant and marker-only controls. Structurally verify that rebind equality uses borrowed fields while preparation still owns its snapshot. No benchmark is required to prove the observed copies; an implementation has not been compiled or run.

This is one bounded cold topology-comparison ownership cause across two Resources modules, priority P3, distinct from ARCH-028's request-time matcher union, ARCH-050's repeated validator invocation, ARCH-061's operator source validation, and ARCH-067's CtxSet transform projection. Repository review/task searches found no existing finding that owns these temporary IpList wire comparisons. No production changes, tests, builds or runtime probes were executed; external consumers were not audited.
