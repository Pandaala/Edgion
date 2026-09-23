---
name: schema-validator-non-k8s-asymmetry
description: Review schema-validation placement and the accepted decision to retain Kubernetes availability checks without live-schema discovery or bundle locking.
knowledge_type: intentional-design
---

# SchemaValidator Non-K8s Asymmetry Is Architectural, Not a Gap

**Audit date**: 2026-05-16
**Linked finding**: c3a-013 (P2, architecture, confirmed → closed, accepted tradeoff)
**Conclusion**: Retain the existing validation placement and availability checks.
The accepted scope does not add live-schema discovery, bundle/channel locks, or
non-Kubernetes GVK/schema compatibility checks. This is a bounded product choice,
not proof that every admitted resource is supported or every schema constraint is covered.

## Why the asymmetry is fundamental

Three write paths in non-K8s mode, with very different control surfaces:

| Path | Can controller gate the write? | Feedback channel |
|---|---|---|
| Admin API (HTTP) | ✅ — request rejected with 400 before persist | Synchronous error |
| `conf_dir/*.yaml` direct write | ❌ — file is on disk before watcher sees event | Status conditions (async) |
| etcd direct put | ❌ — KV in etcd before watcher sees event | Status conditions (async) |

For the two direct-write paths, **whatever validator runs in the watcher cannot prevent
the bad resource from entering the store**. It can only enrich the status conditions
written back. The downstream handler chain (`parse_resource_content` → `handler.validate`
→ `handler.preparse` → `handler.parse` → `handler.update_status`) **already** writes
serde errors, semantic errors, and parse errors to `*.status.yaml` via
`status_handler.write_error_status` / `update_status`. Adding OpenAPI on top would
duplicate most of that, costing CPU at init for negligible marginal coverage.

K8s mode skips local schema loading (`load_schemas` returns
`SchemaValidator::empty()`); the API server gates resource writes using the installed
CRD. Admission does not prove compatibility with Edgion's typed model or runtime.

## Kubernetes availability and compatibility boundary

On 2026-09-23, the user explicitly approved closing
`kubernetes-live-crd-schema-source-and-version-contract` without implementing its
proposed enhancement, after discussing the following scope and tradeoffs:

- Retain Gateway API GVK availability preflight, configured resource watchers,
  watch-error recovery, and initialization readiness deadlines.
- Do not introduce live CRD schema discovery/caching, a bundle/channel compatibility
  startup gate, or an Edgion-owned CRD admission version lock and unlock workflow.
- Do not add the proposed non-Kubernetes GVK, bundle/channel, or schema compatibility
  checks. Existing Admin API schema validation and handler validation remain in place.

Current source evidence:

- `edgion-controller/src/cli/mod.rs::load_schemas` returns an empty validator in
  Kubernetes mode; it does not validate Kubernetes resources against vendored CRDs.
- `conf_mgr/conf_center/kubernetes/version_detection.rs::preflight_check_gateway_api_crds`
  checks required served GVKs, excluding `no_watch_kinds`. Discovery permission and
  transport errors remain errors rather than being treated as absent CRDs.
- `conf_mgr/conf_center/kubernetes/center.rs` runs that preflight before watcher
  startup in both serving modes and bounds initialization with a readiness deadline.
- `conf_mgr/conf_center/kubernetes/controller.rs::run_controllers` starts the
  configured typed watchers. A resource type with zero objects is valid; a missing
  required API type is different from an empty resource list.

The benefit is avoiding duplicate admission machinery and a cluster-wide version
lock that could reject compatible upgrades or constrain other Gateway controllers.
The accepted cost is that served GVK availability does not establish field-level
compatibility: the same HTTPRoute v1 GVK can expose different fields across bundles
and channels. No live-schema compatibility or exact-bundle enforcement is claimed.
The Rust JSON Schema validator does not execute arbitrary Kubernetes CEL.

The local validator still indexes by `ResourceKind`, takes the first CRD version,
and permits duplicate bundle entries to overwrite each other. This closure neither
fixes nor disproves those limitations; the user declined expanding non-Kubernetes
checks in this task. Broader Gateway API compatibility work retains separate
support-matrix, status, and conformance questions. It must honor
this scope decision rather than automatically reintroduce the declined mechanisms.

This decision was checked against current source; no new runtime behavior or
cluster compatibility experiment is asserted. The 2026-05-16 finding above is
historical evidence, not a fresh test result. Closure verification is reported
separately after independent review and project checks.

## What SchemaValidator actually buys

Only one thing, and it is real: **synchronous 400 on the admin API path** for programmatic
clients (CI pipelines, `edgion-cli`, custom tooling). Without it, those clients would need
a "write → poll status → translate conditions → derive error" round-trip. Hence keep.

## Fix suggestions not accepted

- "Inject `SchemaValidator::validate` into `parse_resource_content` / watcher init loop
  for FileSystem and Etcd ingest paths" — cannot prevent the on-disk/in-etcd bad value;
  status reporting via the handler chain already covers it; mostly redundant work.
- "Remove `SchemaValidator` entirely; rely on serde + handler.validate" — regresses admin
  API UX for programmatic callers (sync 400 → async status fetch).
- "Also run schema validation in K8s mode as a defense against CRD/server skew" —
  declined within the scope above. Existing API-server admission is retained;
  schema/runtime skew remains a known limitation, not a disproved possibility.
- "`edgion-cli config lint <file>` as the canonical solution to this finding" — orthogonal
  feature; whether to add a CLI lint subcommand is a separate product decision.

## The one real adjacent problem (out of scope here)

Fields where OpenAPI catches but `serde` and `handler.validate` both miss — `format:
hostname`, numeric `minimum`/`maximum`, `pattern`, certain `enum` strings not modeled as
Rust enums — can pass through and become **silent misconfiguration at runtime** (SNI
never matches, defaults silently applied, etc.). The fix is **handler-side validation
completeness**, not validator relocation. A handler may reuse a bounded generated
contract extracted from the vendored CRD when constraints would otherwise be duplicated
in Rust. RequestRedirect and URLRewrite use that pattern for selected OpenAPI, CEL, and
default semantics. This does not turn the filesystem/etcd watcher into an admission
pipeline: failures still flow through the existing handler/status lifecycle.

## When to re-evaluate

- A concrete same-GVK schema change causes incorrect Edgion runtime behavior, or a
  deployment explicitly requires enforced bundle compatibility. Reassess the
  affected mechanism with evidence and explicit scope approval; do not infer that
  availability checks provide compatibility enforcement.
- A new ingest path appears that **is** gateable (e.g., a write-through admin API for the
  etcd backend) — re-examine whether validation belongs there.
- Programmatic admin API usage drops to zero (e.g., all writes go through K8s or
  edgion-cli stops calling admin API) — removing `SchemaValidator` (option b) becomes
  cheaper.
- Telemetry shows the "silent misconfiguration" class is biting users at scale —
  prioritize the handler-completeness audit, still not validator relocation.
