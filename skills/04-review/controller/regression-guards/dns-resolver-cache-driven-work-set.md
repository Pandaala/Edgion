---
name: dns-resolver-cache-driven-work-set
description: Use when reviewing the Controller DNS resolver's work set, scheduling, apiserver reads, or httpdns LinkSys lookup.
knowledge_type: regression-guards
---

# DNS resolver work set comes from processed caches

## Conclusion and scope

The leader-only Controller DNS resolver
(`edgion-controller/src/services/dns_resolver/service.rs`) never lists Services and
never reads LinkSys objects from the apiserver. Its inputs are:

- **Targets** — `ControllerRuntime::dns_resolver_targets`
  (`services/dns_resolver/targets.rs`). `ServiceHandler::after_publish` upserts the
  processed Service when `decide_action` yields `Resolve` and removes it otherwise;
  `after_unpublish` removes it; `ControllerRuntime::reset` clears the table. Only a
  content change bumps the entry revision and wakes the resolver, so a requeue or
  resync of identical content is free.
- **httpdns owners** — the processed LinkSys cache (`get_cached_any`), which carries the
  ReferenceGrant-checked object and controller-resolved TLS material. mTLS httpdns
  endpoints therefore work in controller mode. `controller_default_resolver(runtime)`
  is the single default-resolver entry point for every conf_center mode.
- **Owned slices** — one label-selected LIST per pass, served from the apiserver watch
  cache (`resourceVersion=0`). The Controller EndpointSlice cache is deliberately not
  used: the operator-configured global `label_selector` applies to every watched kind,
  and resolver-owned slices do not carry operator labels.

Each target has its own next-due instant (`next_refresh`). Due instants sit on one
`RefreshGrid` anchored at loop start with `refresh_interval` spacing, so targets added
at different times share passes instead of fragmenting into per-target phases. A
content change is handled on the next pass, then rejoins the grid. A pass prepares due
targets serially, runs only the DNS queries concurrently
(`MAX_CONCURRENT_RESOLUTIONS`, `RESOLVE_DEADLINE` per query) and publishes each answer
serially as it arrives, so a slow resolver delays only itself. Passes are spaced by at
least `MIN_REFRESH`, which also coalesces bursts of target changes.

No pass resolves or collects until the Service processor is ready: an empty table
during startup must not garbage-collect published slices. The check is repeated after
the target snapshot because `ControllerRuntime::reset` clears processors before the
table, so a snapshot emptied by leadership loss can never reach GC, and again before GC.

Slice correctness (family convergence, fingerprints, ownership proof, guarded deletes)
is unchanged and owned by
[DNS-owned family slice convergence](dns-owned-family-slice-convergence.md).

## Rationale and evidence

ctrl-svc-05 found that every tick (15 s default, enabled by default) performed an
uncached LIST of all Services in scope even with zero ExternalName Services, resolved
targets serially so one slow resolver delayed every other refresh, and issued a raw
`Api::<LinkSys>::get` per httpdns Service per tick, bypassing resolved secret material.
Controller handlers already observe every Service through the watch, so the table is
maintained by the existing serial worker with in-memory writes only (workqueue rule 5),
and IO stays in the leader job.

`after_publish` also runs during initialization, so the table is complete when the
caches report ready; leader-only mode starts leader jobs after caches are ready, and
the readiness check guards the all-serve and restart paths as well.

## Alternatives and limits

- Residual polling: every grid tick (and every change-driven pass) performs one
  label-selected owned-slice LIST per watched namespace from the watch cache, even with
  an empty table; that pass is what collects slices left behind while no leader ran.
- If `Service` is in `no_watch_kinds`, no Service processor exists, so the resolver
  never runs and existing owned slices are not collected. If `LinkSys` is not watched,
  httpdns references fall back to the system resolver.
- With a global `label_selector`, Services outside the selector are no longer resolved
  (previously the LIST ignored the selector). Such Services are also absent from the
  Controller's Service cache, so routes cannot resolve them as backends either.
- DNS TTL-driven refresh is deferred: `edgion_common::dns::resolve` does not surface
  record TTLs. Only `next_refresh` needs to change when it does.
- Resolver-owned slices are not relayed to the Gateway when a global `label_selector`
  excludes them from the EndpointSlice watch (pre-existing, separate issue).

The deferred items (TTL refresh, label-selector relay, residual polling) are tracked in
`tasks/todo/ctrl-svc-05-cache-driven-dns-resolver.md`.

## Verification and re-evaluation

Unit tests: `tests/unit/services/dns_resolver/targets_test.rs` (target classification,
revision/wake semantics, removal paths), `service_test.rs`
(`reconcile_lists_only_owned_slices_from_watch_cache`,
`passes_skip_targets_until_due_or_changed`,
`pass_before_service_cache_ready_neither_resolves_nor_collects`,
`refresh_grid_aligns_due_instants`,
`controller_ticks_reuse_failed_preparation_and_clear_changed_or_removed_services`,
`dns_producer_gates_tenant_and_preserves_default_authority`,
`producer_validation_off_still_rejects_invalid_identity_before_lookup`) and
`handlers/service_test.rs::publication_hooks_maintain_dns_resolver_targets`.
The `run()` loop itself (Notify wakeups, pass spacing) has no direct test; its gating
is covered through `reconcile_with_runtime`. No live Kubernetes run is claimed.

Re-evaluate when Service publication hooks change, when the resolver gains another
input kind, when DNS TTLs become available, or when the global label selector starts
covering controller-written objects.

## Resolution references

- ctrl-svc-05 (`tasks/improve-issues/`, P2 design-error, CONFIRMED and re-verified on
  `feature-10-28`): fixed 2026-11-07 by the cache-driven work set described above.
