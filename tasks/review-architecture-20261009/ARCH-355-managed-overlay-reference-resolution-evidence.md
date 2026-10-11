---
id: ARCH-355
status: confirmed
kind: behavior-defect
priority: P2
partition: PART-474
analyst: /root/analyze_outbound
baseline: 11aeb2347657135da6388057bbf90a4f239e5c16b822a8132cc94916aed7718b
---
# The managed-overlay suite treats structural acceptance as proof that the overlay reference resolved

`ep_resolves_pm_chain` and `pm_validated_accepted` in `edgion-tests/src/client/suites/edgion_plugins/edgion_config_data_managed/edgion_config_data_managed.rs` poll the EdgionPlugins resource and pass when its selected Controller conditions contain `Accepted=True`. The first success message explicitly claims that `overrideRef` resolved correctly; the second case presents acceptance with pre-created ConfigData as evidence of the managed reference chain. Neither predicate checks `ResolvedRefs`, nor verifies that the plugin actually references the expected ConfigData. The preceding ConfigData checks establish that `data-managed` or `data-validated` exists; they do not establish the plugin's reference edge.

The current Controller deliberately separates these facts. `EdgionPluginsHandler::update_status` constructs Accepted from structural/preparse validation, then separately calls `resolve_override_refs` and constructs ResolvedRefs. A missing, disabled, denied, wrong-type or invalid effective overlay can therefore coexist with an accepted safe base. The actual `test_validate_region_route_override_ref_missing` unit requires missing-overlay preparse to remain clean, parse to continue and reference resolution to report `EdgionConfigDataNotFound`. The original ConfigData architecture likewise requires fail-to-base rather than structural rejection. These are supported states, not malformed diagnostic responses.

A concrete regression witness is changing a fixture plugin's `overrideRef.name` to another syntactically valid, absent name while leaving the pre-created expected ConfigData and valid base unchanged. The existence checks still pass and Accepted can remain True while ResolvedRefs is False. All three managed-overlay cases can then pass despite the claimed managed reference chain being broken. The committed local and Kubernetes fixtures both supply explicit expected references in `edgion-default`; neither topology turns Accepted into a reference-resolution condition. This is distinct from ARCH266: these readers already use the current canonical Controller-local conditions projection.

Within these two existing cases, inspect the returned plugin's actual RegionRoute entry and require the expected override reference identity. In the same bounded poll, require both `Accepted=True` and `ResolvedRefs=True` from the already-selected Controller observation. When `metadata.generation` is present, require both conditions' `observedGeneration` to match it; preserve native FileSystem/etcd observations where no generation is supplied. Preserve the existing expected ConfigData presence/type checks, authenticated Admin requests, native and partitioned status handling, retry limits and fail-on-read/parse behavior. Missing conditions must keep the predicate unsatisfied. No production status, fallback, admission, logging, fixture ownership or generic test framework changes are needed.

Checking Accepted alone cannot prove the edge, and checking only ConfigData existence cannot detect a mistargeted reference. A traffic-success check alone would also admit the intentionally valid base fallback. Future verification should feed the real predicates healthy, missing-target, mistargeted-reference, stale-generation and absent-condition observations without changing the existing test identities. This review is static; no suite or cluster was executed.
