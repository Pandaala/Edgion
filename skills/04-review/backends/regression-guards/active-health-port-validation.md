---
name: active-health-port-validation
description: Use when reviewing Service active health-check port overrides, direct resource input, or invalid policy updates.
knowledge_type: regression-guards
---

# Active health-check port validation

## Conclusion and scope

`EdgionBackendTrafficPolicy.spec.healthCheck.active.port` is an optional probe
port override for Service targets. An explicit override must be in `1..=65535`.
Omitted and null values deserialize to `None` and retain endpoint-port inheritance.
The shared `ServiceHealthCheck::get_validation_error` rejects `Some(0)` for all
five probe protocols; serde rejects negative values and integer overflow.

The correction applies to configuration loading and updates. It does not change
the CRD bounds, watcher admission placement, probe transport ownership, or the
separate treatment of encrypted probe TLS errors.

## Rationale and evidence

The declared bounds come from `gwapi_types::{PORT_MIN, PORT_MAX}`, the schemars
annotation on `ActiveHealthCheckConfig.port`, and the committed BackendTrafficPolicy
CRD. `Option<u16>` alone admits zero; schemars annotations do not enforce semantic
validation during serde decoding. Previously the shared validator omitted this
check, so a directly written file or etcd resource could become an eligible
policy with an invalid probe destination.

- `edgion-resources/src/resources/health_check.rs`: reject explicit zero in the
  canonical shared validator while preserving `None`.
- `edgion-resources/src/resources/edgion_backend_traffic_policy.rs`: include the
  error as `healthCheck: port must be >= 1` in spec validation.
- Controller `handlers/edgion_backend_traffic_policy.rs::evaluate_policy`: use
  spec validation for status and complete attachment eligibility. A zero-port
  update clears `resolved_target_class` and conflict participation while retaining
  target dependencies for later repair. Existing resolved ancestor Gateways
  receive `Accepted=False/Invalid`; without an ancestor there is no status entry,
  and missing-target precedence remains unchanged.
- Gateway `backends/policy/backend_traffic/store.rs::prepare_policy`: independently
  revalidate the spec, including when an input carries a valid resolved-class
  marker. Invalid policies publish no effective configuration. Incremental
  replacement withdraws the previous health-check configuration through existing
  background-side-effect reconciliation.
- FileSystem and etcd resource controllers decode direct input into their raw
  stores, enqueue runtime keys, and call the same resource processor from their
  serial workers. The etcd worker writes changed native status with
  `EtcdStatusHandler::write_status_value`; the FileSystem worker writes its
  `.yaml.status` sidecar. This is source evidence for the shared semantic path,
  not a live etcd experiment.

The [schema-validation placement decision](../../resources/intentional-design/schema-validator-non-k8s-asymmetry.md)
accepts asynchronous feedback for direct file/etcd writes, not missing semantic
validation. [Independent active probe transport](../intentional-design/active-health-check-transport-is-independent.md)
continues to own port and TLS selection.

## Alternatives and limits

Treating zero as absence would silently change an explicit invalid value into
endpoint-port inheritance. Adding watcher-side OpenAPI validation would broaden
the fix and contradict the retained validation placement. The shared semantic
check fixes both Controller and Gateway consumers without a second mechanism.

Resource tests exercise YAML and JSON port absence, null, zero, range endpoints,
and representation overflow. The field-contract guard checks the declared bounds
and semantic rejection. Controller and Gateway tests cover invalid publication,
valid-to-zero withdrawal, and recovery. The FileWatcher integration test writes
the configuration directly and polls processed eligibility and persisted status
across valid, zero, omitted, null, and repaired generations.

These tests do not establish live etcd or Kubernetes admission equivalence, nor
do they issue actual network probes to boundary ports 1 and 65535.

## Verification and re-evaluation

Verification on 2026-10-06 passed:

- `cargo test -p edgion-resources --lib resources::health_check`: 12 tests.
- `cargo test -p edgion-resources --test crd_field_contract_parity`: 8 tests.
- `cargo test -p edgion-controller --lib conf_mgr::sync_runtime::resource_processor::handlers::edgion_backend_traffic_policy::tests`:
  25 tests, including attachment withdrawal and recovery.
- `cargo test -p edgion-gateway --lib backends::policy::backend_traffic::store::tests`:
  27 tests, including independent eligibility checks and incremental withdrawal.
- `cargo build -p edgion-controller --bin edgion-controller` and
  `bash edgion-tests/integration/scripts/integration/test_file_watcher.sh`:
  the complete FileWatcher group passed, including direct-write phases
  `9090 -> 0 -> omitted -> null -> 65535`. Each phase checked both processed
  resource generation and persisted `Accepted.observedGeneration` to prevent
  an unchanged inherited-port value from passing against the previous phase.
- `cargo fmt --all`, `cargo check --workspace --all-targets`, and
  `cargo clippy --workspace --all-targets`; agent-document consistency,
  SSA-force, metrics inventory, Pingora source policy, Gateway API transform
  contract, and unit-test layout guards all passed.
- Independent read-only review approved the correction after the FileWatcher
  generation checks were added. `bash -n` and `git diff --check` also passed.

No full `cargo test --all`, live etcd test, or Kubernetes cluster test was run.
Re-evaluate if the override range or inheritance semantics change, if policy
validation stops sharing this validator, or if invalid policy updates gain a
different publication/withdrawal contract.

## Resolution references

- `health-check-zero-port-validation` — confirmed semantic-validation defect;
  corrected in the shared health-check validator.
