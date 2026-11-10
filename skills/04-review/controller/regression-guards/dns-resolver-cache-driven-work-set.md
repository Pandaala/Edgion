---
name: dns-resolver-cache-driven-work-set
description: Use when reviewing the Controller DNS resolver for controller-mode ExternalName Services — its work set, scheduling, httpdns LinkSys lookup, in-memory synthetic EndpointSlice publication, or its lifecycle.
knowledge_type: regression-guards
---

# Controller DNS resolver: cache-driven work set and in-memory publication

## Conclusion and scope

The Controller DNS resolver (`edgion-controller/src/services/dns_resolver/`) resolves
only `ExternalName` Services explicitly annotated `edgion.io/dns-resolution-mode:
controller` (parsed by the shared `DnsResolutionMode`; absent, other or invalid values
leave the Service to the Gateway). It never lists Services, never reads LinkSys objects
from the apiserver and never writes to the apiserver. Its inputs and outputs:

- **Targets** — `ControllerRuntime::dns_resolver_targets` (`targets.rs`).
  `ServiceHandler::after_publish` upserts the processed Service when it is a target and
  removes it otherwise; `after_unpublish` removes it. Only a content change bumps the
  revision and wakes the resolver, so a requeue of identical content is free.
- **httpdns owners** — the processed LinkSys cache (`get_cached_any`), carrying the
  ReferenceGrant-checked object and controller-resolved TLS material. A tenant
  `dns-resolver-ref` is used only when `dns_query_allowed` grants it; the administrative
  default (`dns_resolver.link_sys_ref`) keeps its own authority.
  `controller_default_resolver(runtime)` is the single default-resolver entry point.
- **Publication** — answers become synthetic EndpointSlices built by the shared
  `edgion_common::dns::endpoint_slices` builder (`edgion.io/endpoint-origin:
  controller-dns`, one per family) held in `ControllerRuntime::dns_synthetic_slices`.
  Changed keys are enqueued with `processor_registry.requeue("EndpointSlice", key)`; the
  EndpointSlice processor reads them through `ProcessorHandler::synthetic_object` when
  its real source has no object for the key (a sourced object wins), so workqueue rule 3
  holds and conf_sync relays them like watched slices. An unchanged answer enqueues
  nothing (`DnsSyntheticSlices::replace_service` returns only changed keys).

Scheduling (`service.rs`, `ResolverState`): each target has its own next-due instant on
a fixed `refresh_interval` (see
[fixed-interval refresh](../intentional-design/dns-resolver-fixed-interval-refresh.md)).
A revised target is handled on the next pass: its last good answer is republished at
once with the new ports / filtered by the new family, then re-queried. Queries run
concurrently (`MAX_CONCURRENT_RESOLUTIONS`, `RESOLVE_DEADLINE`) and each answer is
applied as it arrives. Passes are spaced by `MIN_REFRESH`, coalescing bursts. Success
replaces the set, `NoAddresses` withdraws it, other failures keep the last good set; an
invalid family annotation withdraws without querying. Removed targets, and synthetic
slices whose Service is no longer a target, are withdrawn on the next pass.

Lifecycle: the task belongs to the runtime (`DnsResolverTask`). `ServiceHandler` starts
it with the first target (`ensure_dns_resolver_running`); `ControllerRuntime::reset`
aborts it and clears targets and synthetic slices. It is not a leader job: slices are
per-process memory, so every process that runs the resource pipeline (Kubernetes
all-serve followers, file-system and etcd centers) resolves for itself.

## Rationale and evidence

ctrl-svc-05 (fixed 2026-11-07) introduced the cache-driven work set: previously every
tick listed all Services and issued a raw LinkSys GET per httpdns Service. The
2026-11-10 DNS resolution redesign (`tasks/todo/dns-resolution-redesign/`) then removed
apiserver publication. That fixed: Kubernetes-only operation, a global `label_selector`
excluding resolver slices from the EndpointSlice watch, the per-tick owned-slice LIST
and orphan GC, owner-reference / SSA ownership proofs, and EndpointSlice write RBAC for
the resolver (the Admin API still needs it). Handlers keep in-memory writes only
(workqueue rule 5); DNS IO stays in the resolver task.

## Alternatives and limits

- If `Service` is not watched no targets exist; if `EndpointSlice` is not watched the
  requeue finds no processor and nothing is relayed. If `LinkSys` is not watched,
  httpdns references fall back to the system resolver.
- Resolver slices are not visible through `kubectl`; inspect them through the Controller
  admin/conf_sync views of EndpointSlice.
- Each replica resolves independently; replicas may briefly publish different answers
  for rotating records.
- Rejected: a separate synthetic kind or a second EndpointSlice workqueue (rule 1), and
  writing the synthetic object into the processor cache directly (bypasses the handler
  and its dependent requeues).

## Verification and re-evaluation

Unit tests: `tests/unit/services/dns_resolver/{targets_test,service_test,synthetic_test}.rs`
(target classification, revision/wake, plan/apply publication, change suppression,
withdrawal paths, family narrowing, ReferenceGrant gating, httpdns preparation reuse),
`handlers/endpoint_slice_test.rs::synthetic_dns_slices_enter_the_pipeline_only_when_unsourced`
and `handlers/service_test.rs::publication_hooks_maintain_dns_resolver_targets` (start
and reset). Integration: `DnsResolution::controller_mode_publishes_endpoints` in the
file-system harness. The `run()` loop itself has no direct test.

Re-evaluate when Service publication hooks change, when the resolver gains another
input kind, or when the EndpointSlice processor changes how it reads its source.

## Resolution references

- ctrl-svc-05 (`tasks/improve-issues/`, P2 design-error): fixed 2026-11-07 by the
  cache-driven work set.
- DNS resolution redesign (2026-11-10): in-memory publication, explicit opt-in,
  runtime-owned lifecycle; supersedes
  [DNS-owned family slice convergence](../superseded/dns-owned-family-slice-convergence.md).
