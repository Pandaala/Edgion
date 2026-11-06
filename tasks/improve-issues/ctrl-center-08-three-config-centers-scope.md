# ctrl-center-08: Three production config-center backends, each with its own HA, status and write path

- **Category**: over-design
- **Severity**: P3
- **Scope**: large-direction
- **Location**:
  - `edgion-controller/src/conf_mgr/conf_center/kubernetes/` (~6.7k lines), `etcd/` (~3.2k), `file_system/` (~3.9k), shared `common.rs`/`non_kube_common.rs`/`traits.rs`/`source_query.rs` (~2.2k)
  - `edgion-controller/src/conf_mgr/conf_center/etcd/leader_election.rs`, `leadership_gate.rs`, `status.rs`, `storage.rs`, `watcher.rs`, `resource_controller.rs`

## Summary

The middle of the pipeline (workqueue, processors, ServerCache, conf_sync) is shared, but each
backend carries its own ingestion, status persistence, write/CAS path and HA story: Kubernetes (Lease,
two HA modes), etcd (v3election + self-managed keepalive + leaseless sweeper + pre-write gate, with an
accepted unfenced stale-leader write window), FileSystem (single instance). For a product positioned as
a Kubernetes Gateway API implementation, etcd is a second full production control plane whose
semantics differ from Kubernetes (status stored as YAML keys, no admission, no RBAC per resource,
active-passive only).

## Current behavior (evidence)

- etcd mode reimplements election (`leader_election.rs`, ~580 lines with keepalive driver, guard txn,
  candidate refresher, leaseless sweeper), a leadership gate, YAML status keys, CAS storage, a
  prime/dispatch/compaction-resync watcher and a resource controller — none shared with Kubernetes.
- `skills/04-review/concurrency/intentional-design/etcd-leadership-gate-not-fenced-at-txn.md` records
  that closing the stale-leader write window is deferred to "the HA work"; the Kubernetes mode has a
  different, also separately reasoned, status-write guard.
- Every cross-cutting change (init phases, readiness, status pipeline, reference managers, decode
  recovery) has to be designed and regression-guarded three times (see the many backend-specific
  regression-guard pages under `controller/` and `concurrency/`).

## Industry comparison

- Istio: Kubernetes API only; non-K8s sources arrive through MCP into the same model rather than as
  separate storage backends.
- APISIX: etcd only (plus a standalone YAML mode), one HA story.
- Kong: DB (Postgres) or DB-less declarative; the Kubernetes Ingress Controller translates CRDs into
  DB-less config rather than adding a third backend.
- Envoy Gateway: Kubernetes provider plus a file provider for standalone; no KV-store backend.

## Impact

Large maintenance and review surface (about a third of the unit is backend-specific HA/status code);
slower evolution of shared lifecycle semantics; HA correctness must be proven separately for etcd.

## Suggested direction

Options:
1. Keep Kubernetes + FileSystem only (FileSystem already plays the Kong DB-less / Envoy Gateway file
   provider / APISIX standalone role); drop etcd.
2. Keep etcd only if non-Kubernetes HA is a committed product target (e.g. EdgionCenter-managed bare
   metal), and then converge its lifecycle on one shared "single-writer source" abstraction with
   Kubernetes so readiness/status/HA are implemented once.

Recommendation: make an explicit product decision; absent a committed non-K8s HA customer, option 1.

## Notes

`conf-sync/intentional-design/conf-center-etcd-abstraction.md` documents the etcd bootstrap/TLS boundary,
not whether the backend should exist. This is a scope question, filed P3 for the owners' decision.

## Verification (independent)

- **Verdict**: CONFIRMED
- **Severity**: P3
- `edgion-controller/src/conf_mgr/conf_center/{kubernetes,etcd,file_system}/` each carry their own election/status/storage code.
- `skills/04-review/concurrency/intentional-design/etcd-leadership-gate-not-fenced-at-txn.md` defers the etcd stale-leader window to future HA work.
- `conf-sync/intentional-design/conf-center-etcd-abstraction.md` covers bootstrap/TLS, not whether the backend should exist. Kept at P3 as an owner decision, no defect claimed.

## Additional evidence (wave 3)

- The two local backends already share `LocalControllerBackend`/`spawn_all_controllers`
  (`non_kube_common.rs:410-614`), yet their per-kind controllers are near-copies:
  `file_system/resource_controller.rs` (~1.0k lines) and `etcd/resource_controller.rs` (~0.7k
  lines) each re-implement the same init/apply-as-init branch, identity-mismatch tombstoning,
  `RuntimeInvalidEnvelopes`, error-sidecar clearing and the full worker loop with inline status IO.
  Divergence already exists (FS re-reads the file in the worker, etcd uses the raw store), and
  fixes such as worker supervision (see `ctrl3-02`) must be made twice.
- The Admin API "bounded source" read path is implemented three times with different cost models
  (`kubernetes/source_query.rs`, `etcd/source_query.rs`, `file_system/source_query.rs`, plus the
  shared cursor/precondition module `conf_center/source_query.rs`). The FileSystem variant walks
  and parses every file of the kind for each 10-item page and for each count
  (`file_system/source_query.rs:217-383`) although `PathRegistry` already indexes every key, so
  paging through N objects costs O(N²) file reads. Each backend additionally keeps a legacy
  unbounded `get_list_by_kind(None)`/`list_all` path (`traits.rs:320-358`), still used by
  `diagnostics_handlers.rs:63` (`center_list_resources(EdgionPlugins, None)`).

## Re-verification (feature-10-28)

- **Verdict**: STILL-VALID
- **Severity**: P3 (unchanged)
- `edgion-controller/src/conf_mgr/conf_center/etcd/` still carries its own `leader_election.rs`, `leadership_gate.rs`, `status.rs`, `resource_controller.rs`, `source_query.rs`; changes since `feature-08-30` are dependency-migration edits (`common.rs`, `status.rs`, `storage.rs`).
- `traits.rs:320,358`: unbounded `get_list_by_kind`/`list_all` remain; `api/diagnostics_handlers.rs:63` still calls `center_list_resources(EdgionPlugins, None)`.
- No product decision on dropping etcd recorded in `skills/` or `tasks/`.
