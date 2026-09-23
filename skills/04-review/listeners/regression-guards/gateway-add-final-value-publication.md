---
name: gateway-add-final-value-publication
description: Gateway Add publication must use the final compressed resource even when its runtime key already exists.
knowledge_type: regression-guards
---

# Gateway Add final-value publication

## Conclusion and scope

GatewayHandler's Add branch replaces the raw Gateway through the existing
update_gateway operation. CacheData classifies a present key as Add when its
queued history contains InitAdd or EventAdd; that label does not establish
absence from GatewayStore. The accepted final value must supply the subsequent
listener-info and TLS-matcher rebuilds, matching the prospective PROXY policy
snapshot and backend TLS/plugin publication inputs.

This is a low-priority receiver consistency correction. Current Controller
ResourceProcessor::save publishes EventUpdate at runtime; initialization uses
InitAdd in the server cache and List/full_set on the Gateway. Inspection found
no normal Controller producer of repeated EventAdd for Gateway. Do not describe
this as a reproduced normal Controller hot-reload outage.

## Rationale and evidence

- `edgion-gateway/src/conf_sync/cache_client/cache_data.rs`:
  process_compressed_events selects the final cached value and classifies it
  from event history. Two Adds in one window starting empty yield one final Add.
  Separate-window Adds or Delete/Add can reach an already populated runtime key.
- `edgion-gateway/src/runtime/handler.rs`: prospective_gateway_snapshot inserts
  Add values as replacements. Previously the Add branch updated backend TLS,
  then add_gateway rejected an existing key; list_gateways supplied old data
  to derived rebuilds while plugin publication used the incoming resource.
  store_added_gateway now replaces the raw value through update_gateway.
- `edgion-controller/src/conf_mgr/sync_runtime/resource_processor/processor.rs`:
  save uses EventUpdate; startup uses InitAdd. ServerCache preserves the runtime
  event type. The receiver's support for Add is not proof of a production sender.

The [ConfHandler guidelines](../../../01-architecture/02-gateway/11-conf-handler-guidelines.md)
remain applicable. No generic compression or public trait contract is changed.

## Alternatives and limits

Global Add-to-Update normalization would affect unrelated handlers unnecessarily.
Rejecting only the raw-store write cannot reject an entire publication whose
other stores are already being updated. The local replacement adds no state or
request-path work. RejectFatal still returns before business-store writes, and
the unchanged listener-key comparison still gates route-manager rebuilds.

The accepted [sequential publication window](../../concurrency/intentional-design/partial-update-inconsistency-windows.md)
is distinct from the former persistent generation split. This correction does
not promise atomic cross-store publication or change the
[Gateway recreation restart requirement](../intentional-design/gateway-recreation-requires-restart.md).
Full-set omission of backend identities remains governed by that lifecycle rule.
User configuration, schema and supported lifecycle documentation are unaffected.

## Verification and re-evaluation

The local-store regression calls the same private write function as the Add
branch and checks the changed Gateway in list_gateways, the rebuild input.
The CacheData regression exercises same-window and separate-window Adds,
Delete/Add, Update, Remove, reset and empty flush using the existing test seam.
These tests do not execute the global publication graph or claim wire-level
Gateway recreation coverage.

Verification in this run passed both targeted tests via
`cargo test -p edgion-gateway --lib <test-name>`:
`repeated_add_replaces_gateway_used_for_rebuild` and
`compressed_events_publish_final_values_without_reclassifying_adds`.
Independent read-only review returned LGTM, including the final test-only
parking_lot mutex correction. `cargo fmt --all`,
`cargo check --workspace --all-targets`, and
`cargo clippy --workspace --all-targets` passed; the final clippy run was clean.
Agent-doc validation, SSA force, metrics inventory, Pingora source policy and
Gateway API transform contract checks also passed. No full test suite or wire
integration test was run. An initial clippy warning about the test recorder's
standard mutex was corrected; the affected test, check and clippy were rerun.

Re-evaluate if Controller event classification changes, another production Add
source appears, the Add branch stops using the tested write function, or Gateway
dynamic lifecycle support is explicitly authorized.

## Resolution references

- `replace-gateway-generations-on-coalesced-add`: approved bounded receiver
  consistency fix; normal production trigger not established.
