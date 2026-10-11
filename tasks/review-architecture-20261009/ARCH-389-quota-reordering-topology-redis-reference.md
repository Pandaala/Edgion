---
id: ARCH-389
status: confirmed
kind: behavior-defect
priority: P2
partition: PART-508
analyst: /root/analyze_outbound
baseline: 93910657a31737c17726558f707ccce0e901fd11b669f0e948ef71b37a90f339
---

# Preserve the selected topology's Redis reference during quota reordering

`test_independent_quotas_and_final_header_ownership` in `rate_limit_redis.rs` unconditionally embeds the local `06_EdgionPlugins_multi-quota.yaml`. It reverses that document's quotas, changes `responseQuota`, applies it through the Controller Admin API, and subsequently restores the same local document. In the supported Kubernetes invocation, both publications replace the declared Kubernetes Redis reference with the local reference.

The local document uses `default/redis-test`; the Kubernetes document deliberately uses `edgion-default/redis-test`. The current Kubernetes Redis LinkSys has the latter identity and points at the in-cluster Dragonfly service. The Kubernetes runner selects RateLimitRedis, publishes its own conf tree, and invokes the shared static suite without excluding this case. The batch helper exports `EDGION_TEST_K8S_MODE=true`. `TestContext::apply_yaml` transmits the supplied document unchanged; it does not adapt Redis references to the active topology.

Consider the supported Kubernetes setup after this case's original five requests have succeeded. Applying the reordered local document changes only the intended quota ordering/response selection plus an unintended Redis owner. The canonical LinkSys runtime lookup uses an exact namespace/name key; it has no alias from `default/redis-test` to `edgion-default/redis-test`. RateLimitRedis's current configuration selects `onRedisFailure: Deny`. After that generation is published, fresh convergence probes cannot obtain the required successful 200/limit4 response from the declared healthy Redis owner. The later restoration also retains the wrong reference. A separately preseeded `default/redis-test` can mask the defect; the claim concerns the repository's default Kubernetes fixtures, not an observed cluster incident.

Select the existing local or Kubernetes multi-quota source using the harness's current topology signal before extracting its first document. Derive both the reordered document and restoration document from that one selected source. Change only quota ordering and `responseQuota` for the transition; preserve the selected Redis reference, resource identity, namespace, request timeout, key sources and policy names. Capturing the actual initial resource through the authenticated Admin reader is an alternative when testing arbitrary custom fixture roots, but this case does not need a new fixture registry or converter.

Retain the five-request tenant/user ledger, final rejection headers/body, fresh convergence identities, bounded poll, preserved exhausted named bucket, and all existing case identities. Do not copy the local reference into Kubernetes, skip the case, weaken its convergence assertion, change Redis lookup/failure policy, or introduce counter refunds. ARCH264 separately owns restoration on failed/cancelled transitions; this correction chooses the right source even on an otherwise successful path. ARCH344 concerns missing retry-quota declarations, whereas both fixtures here already exist. No build, test, network operation or mutation occurred.
