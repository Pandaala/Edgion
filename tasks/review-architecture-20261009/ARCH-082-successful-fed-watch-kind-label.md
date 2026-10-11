---
id: ARCH-082
status: confirmed
kind: behavior-defect
priority: P2
partition: PART-428
analyst: /root/analyze_outbound
baseline: 22f0fb8b6f4353f689ca4ddc172a4dfd98a2d2929dd1709240cbe4a1b3796471
---

Title: Project successful federation watch metrics onto the canonical kind domain

The successful federation watch path normalizes a request kind for authorization and storage, then labels its event counter with the original request spelling. This violates the existing bounded, compile-time kind-label contract and splits one resource's watch activity across spelling-dependent series.

In `edgion-controller/src/fed_sync/fed_client/mod.rs:701`, `normalize_kind(&req.kind)` produces the canonical kind used by the Center authorizer, `list_sync_cached`, and `css.watch`. A denied request already projects that value through `authz_classifier::bounded_kind_label` before emitting RBAC metrics. After a successful watch, however, line 804 captures `req.kind.clone()` as `watch_kind`, and lines 826–829 pass it to `fed_metrics::record_watch_event`. The Common helper emits that string directly as the `kind` label; it does not normalize or restrict it.

A concrete accepted witness is `HTTPRoute` versus `httproute` or `hTtPrOuTe`. `ResourceKind::from_kind_name` lowercases these spellings and returns the same HTTPRoute kind. `normalize_kind` produces `HTTPRoute`, so the default Center watch policy authorizes them identically and the ConfigSyncServer selects the same canonical watch object. A delivered update nevertheless increments a different metric series for each spelling. Existing aliases for other recognized kinds can similarly produce noncanonical labels. Unknown resource strings and arbitrary whitespace are not needed for this witness: successful store lookup still restricts the operation to a registered kind.

The global cardinality guard does not make this label domain correct. Enforce mode rejects new keys after its admission threshold, with the guard's documented concurrent overshoot, so this is not a claim of unlimited exported series under default settings. Distinct accepted spellings still fragment canonical kind aggregates and consume separate cardinality-admission slots that should represent the same resource. No new metric or additional request observation is required.

Capture the existing `bounded_kind_label(&kind)` projection once when constructing the successful watch task and pass that static label to the existing counter. Keep the normalized owned kind for authorization/store operations and preserve the original request ID and protocol responses. Do not change accepted kind spellings, RBAC semantics, watch versions, replacement cancellation, event counts, direction labels, or the global cardinality guard. This is a local reuse of the already-owned canonical projection, not a new metric taxonomy or normalization helper.

The existing federation metric tests pin enum labels and exercise a canonical `EdgionConfigData` event label; classifier tests verify lowercase route-kind normalization. The federation client tests cover denied watch admission, but do not test the label produced by an accepted noncanonical watch spelling. A focused regression should exercise the successful forwarding producer with canonical and accepted alternate spellings and establish one canonical label, including when a recorder has a finite cardinality cap. No tests or applications were run in this read-only review.

This finding is distinct from cached request-metric handle eviction (ARCH-043), process-state sampling (ARCH-078), and federation TLS ownership (ARCH-074/075). It concerns the existing successful-watch producer's failure to use an already available canonical bounded label.
