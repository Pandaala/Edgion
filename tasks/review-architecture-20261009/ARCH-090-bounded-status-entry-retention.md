---
id: ARCH-090
status: confirmed
kind: architecture-debt
priority: P3
partition: PART-429
analyst: /root
baseline: 22f0fb8b6f4353f689ca4ddc172a4dfd98a2d2929dd1709240cbe4a1b3796471
---

# ARCH-090: Own bounded status-entry retention during snapshot collection

## Current ownership and concrete scenario

`edgion-controller/src/observe/status_metrics.rs::Renderer` accepts an unrestricted `&mut Vec<Entry>`. `render_kind` appends every resource that has an abnormal condition or exceeds the condition-array threshold. `snapshot_once` invokes every registered renderer, sorts the complete vector, and only then reads the configured hard cap and truncates (`469-500`). The cap correctly limits the published entry count and metric output cardinality. It does not limit collection-time retention; truncating a Vec also leaves its backing capacity unchanged before that Vec is published through ArcSwap.

Consider one supported background tick with N qualifying current resources and a configured cap K smaller than N. Store iteration constructs N owned entries, including identity strings and trigger vectors, although only the first K sorted entries will be exposed. The complete vector's allocation can remain on the published snapshot even after discarded entries' payloads are dropped. This is scale-dependent temporary/materialized state, not a leak or a claim of observed exhaustion. It is unrelated to Gateway request processing or an individual small cold resource clone.

The actual CLI installs the validated startup cap and starts this periodic loop. FileSystem, etcd and Kubernetes controllers register their authoritative RawStore views; the dedicated metrics router reads the published snapshot. No producer needs all N metric entries simultaneously to provide the existing capped output. RawStore snapshots and per-resource JSON/condition analysis remain separate costs.

## Bounded improvement and value

Give this module's collection seam a private bounded-entry owner rather than an unrestricted vector. Count all eligible entries, retain exactly the first K under the existing complete derived Entry ordering, and publish a sorted vector containing those retained entries with capacity proportional to K. A bounded max-heap is one feasible implementation; sort only the retained result. Preserve duplicate entries and the full ordering, including condition vectors and max_conditions_len tie breakers. Do not stop scanning after K entries: RawStore and renderer iteration order is unspecified and later entries may precede earlier ones.

Keep the existing single aggregate cap warning with the true pre-cap count, and publish only after all renderers finish successfully. Read the startup-fixed cap once for the collection generation. This removes discarded-entry retention from the interface and makes future renderers unable to bypass the existing retention owner. It does not create a resource admission limit, truncate an individual label payload, or guarantee a process heap/byte bound.

The neighboring `unknown_fields_registry::BoundedCollector` already separates total counts from deterministic bounded retention. Its narrow identity ordering and path-bounding policy are different from Entry ordering; copying its implementation unchanged would alter this metric's selection contract. Reuse the bounded-retention principle within the existing status owner. Introduce a generic shared abstraction only if an actual identical consumer contract justifies it, not a new registry framework.

## Alternatives, invariants and migration seams

Keeping sort-then-truncate is simple and correct for cardinality, but retains all qualifying entries unnecessarily during each capped generation. Removing sorting or retaining the first visited K changes deterministic selection. Moving the scan into resource mutation hooks introduces incremental synchronization and recovery/deletion ownership that the current full-replacement design intentionally avoids. Changing RawStore's public API, JSON projection, condition vocabulary, label limits or recorder cardinality policy is outside this finding.

The bounded change is local to Renderer, render_kind, snapshot_once and their direct unit fixtures. Preserve periodic complete-source scanning, same-kind renderer replacement/drop on reload, current condition classification and scan order, both err/warn triggers, bloated clean resources, metric names/labels/escaping, zero-series healthy output, configured ranges, panic propagation into the existing outer catch_unwind, previous snapshot on failure, and atomic publication. Add no metrics or configuration, no raw configuration logs, and no request-path work.

## Original knowledge, tests and future verification

Original metrics guidance and inventory explicitly define a capped hand-rendered cardinality surface. They do not promise a heap bound; this report does not allege breach of one. The source-linked historical design file is absent from the current tree. The current resource-abnormal guide preserves full rebuilding and sorted atomic publication; its per-renderer panic-isolation wording is stale because a panic aborts the entire tick and leaves the previous snapshot. Do not silently broaden that isolation guarantee while changing collection.

The CPU/memory review method and small-cold-clone rejection were checked. This recommendation concerns complete fleet materialization after an explicit cap, not isolated resource copies, claimed latency, or an invented incident. Existing full status tests exercise classification, escaping, registration replacement, threshold behavior, panic propagation and a historical timing assertion; they do not exercise over-cap selection/retention. Full unknown-field tests demonstrate deterministic bounded collection and honest totals but are not evidence that the status collector is bounded today. No tests or benchmarks ran.

Future tests should compare retained results with the existing full-sort-then-truncate result under shuffled renderer/store order, below/equal/above cap, duplicates, all ordering tie breakers and an eligible count much larger than the cap. Inspect actual retained vector/collector capacity, count and warning behavior; verify a renderer panic preserves the old snapshot and reload replacement still releases the old store. Retain classification and threshold regressions. Whole-RawStore Arc snapshots, one-resource JSON, condition vectors and scrape serialization remain outside the entry-count bound.
