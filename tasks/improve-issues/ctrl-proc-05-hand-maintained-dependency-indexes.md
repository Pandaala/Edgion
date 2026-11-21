# ctrl-proc-05: About 20 hand-maintained reverse indexes with ad-hoc string keys replace one generic dependency graph

- **Category**: over-design
- **Severity**: P2
- **Scope**: large-direction
- **Location**:
  - `edgion-controller/src/conf_mgr/runtime.rs:34-111` (the state fields), `runtime.rs:213-286` (`reset`)
  - `edgion-controller/src/conf_mgr/sync_runtime/resource_processor/ref_manager.rs` (`BidirectionalRefManager`)
  - `.../service_ref.rs:24-46` (`request-mirror:` key prefix), `.../frontend_ca_ref.rs:32` (`{kind}:ns/name`),
    `.../handlers/gateway.rs:347,654,812` (`Gateway:ns/name`), `.../handlers/backend_tls_policy.rs:36` (`ns/svc::section`)
  - `.../gateway_route_index.rs`, `.../attached_route_tracker.rs`, `.../delegation_graph.rs`,
    `.../listener_port_manager.rs`, `.../proxy_protocol_listener_policy.rs`, `.../edgion_config_data_utils/ref_manager.rs`

## Summary
Cross-resource dependency tracking is spread over about 20 independent stateful structures on
`ControllerRuntime`. Examples are `secret_ref_manager`, `service_ref_mgr`, `backend_ref_mgr`,
`cross_ns_ref_mgr`, `frontend_ca_ref_manager`, `plugin_ref_mgr`, `stream_plugin_ref_mgr`,
`link_sys_ref_mgr`, `edgion_config_data_refs`, `gateway_route_index`, `attached_route_tracker`, two
`DelegationGraph`s, `listener_port_manager` (including its own `blockers` ref manager),
`proxy_protocol_listener_policy`, and the BTP/EBTP policy states. Each one has its own key-string format.
Every handler registers edges manually in `parse`/`on_change` (45 `add_ref` sites) and clears them in
`on_delete` (about 100 `clear_*` sites). Correctness rests on per-handler discipline ("clear-before-add",
"requeue dropped keys", "register before snapshot read"). Each rule is documented separately, and several
have already produced bugs.

## Current behavior (evidence)
- `ServiceRefManager` is a type alias of `SecretRefManager` (`service_ref.rs:14`). Mirror edges are told
  apart by a `request-mirror:` string prefix and filtered by `starts_with` (`service_ref.rs:24-46`).
  BackendTLSPolicy registers `ns/svc::section` keys that route-side helpers had to reduce back to the base
  key (`backend_tls_policy.rs:36`). EdgionConfigData uses `Gateway:ns/name` source strings
  (`gateway.rs:347,812`).
- Bugs caused by key-format or edge mismatches are recorded as regression guards:
  `skills/04-review/handler-requeue/regression-guards/btp-section-key-requeue-gap.md` (bare key vs
  `::section` key), `ebtp-typed-service-key-btp-requeue.md` (key translation between two policy indexes),
  `reference-grant-dependency-before-authorization.md` (edge registered after the read), and the
  `handler-requeue-rule.md` mandatory page (a handler forgot GatewayRouteIndex registration, which left
  routes permanently stale).
- Every new consumer must also extend `ControllerRuntime::reset()` by hand (`runtime.rs:213-286`). A missing
  entry leaks edges across leader epochs.
- The same "which resources depend on X" query is answered differently per kind. For example,
  `resolve_policy_ancestor_gateways_for_targets` (`handlers/mod.rs:144-177`) joins three separate
  indexes, and `policy_ancestor_admission.rs:136` falls back to a full raw-store scan.

## Industry comparison
- controller-runtime: dependencies are declared once per controller with
  `Watches(source, EnqueueRequestsFromMapFunc)`, backed by **cache field indexers** (`IndexField`). The
  index is derived from the cached object, so it can never drift from the object and needs no manual clear
  on delete.
- Istio pilot: each config is reduced to a set of `ConfigKey{Kind, Namespace, Name}` dependencies.
  `PushRequest.ConfigsUpdated` is matched against them generically (`DependsOnConfig`/`SidecarScope`
  dependency maps). There is one typed key and one mechanism.

## Impact
Every new resource or reference type adds another structure, another key format, another `reset` line and
another set of handler hooks. Review knowledge must keep per-edge guards, and the class of bug "edge
missing or stale → dependent never reconciled" keeps recurring. Ad-hoc string keys also defeat the type
system: a key-format mismatch compiles fine and fails silently.

## Suggested direction
Options:
1. **One typed dependency graph owned by the processor (recommended).** Each handler returns its
   dependency set from `parse` as a typed set, for example
   `Vec<Dependency { kind: ResourceKind, key: ObjectKey, edge: EdgeClass }>` where `EdgeClass` is
   Forwarding/Mirror/TLS/CA/Ownership/… instead of string prefixes. The generic `ResourceProcessor` then
   atomically replaces the owner's edge set after a successful reconcile, removes it on delete, computes
   the dropped set (so "requeue dropped dependents" becomes generic), and clears everything in `reset`.
   Event fan-out (`Secret` → owners, `Service` → owners, …) becomes one lookup by `(kind, key, edge filter)`.
   Specialized *derived* state that is not a reverse index (listener conflicts, attachment counts,
   delegation levels) stays specialized.
2. Incremental option: keep `BidirectionalRefManager` but replace `String` keys with a typed `ObjectKey`
   plus `EdgeClass`, and move registration and clearing into the processor. This removes the key-format bug
   class without the full redesign.

## Notes
- `skills/04-review/cpu-memory/review-evidence/bidirectional-ref-manager-no-gc-not-memory-leak.md` and
  `controller/regression-guards/reference-manager-canonical-surface.md` cover memory and API naming of the
  existing manager, not the multiplicity and drift risk.
- `skills/01-architecture/00-common/06-resource-reference-contract.md` defines *which* references are legal (default profiles, convenience manifest, grant
  boundaries) and when a dependency edge is registered. It does not cover how dependency edges are
  stored. The two can be done independently, but its per-field owning adapters
  (`edgion-resources/src/resources/refs/`) are a natural source for the typed `EdgeClass` list.

## Verification (independent)

- **Verdict**: CONFIRMED
- **Severity**: P2
- `resource_processor/service_ref.rs:14-46`: `ServiceRefManager = SecretRefManager`; mirror edges distinguished by `request-mirror:` prefix and `starts_with` filter.
- `skills/04-review/handler-requeue/regression-guards/btp-section-key-requeue-gap.md`, `ebtp-typed-service-key-btp-requeue.md`, `reference-grant-dependency-before-authorization.md` record key-format/edge-ordering bugs of exactly this class.
- `edgion-controller/src/conf_mgr/runtime.rs` `reset` must be hand-extended per structure.
- P2 kept: large refactor, justified by a recurring correctness bug class rather than style.

## Re-verification (feature-10-28)

- **Verdict**: STILL-VALID
- **Severity**: P2 (unchanged)
- `runtime.rs`, `ref_manager.rs`, `service_ref.rs`, `frontend_ca_ref.rs`, `gateway.rs` unchanged since `feature-08-30`; `ServiceRefManager = SecretRefManager` with `request-mirror:` prefix still in `service_ref.rs:14-46`.
- `backend_tls_policy_ref.rs` and BTP/EBTP handlers changed only `DateTime<Utc>` → `jiff::Timestamp`; key formats unchanged. `reference_grant.rs` shrank (-60 lines) but kept the hand-maintained ref managers.
