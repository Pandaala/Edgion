---
name: otlp-masking-and-runtime-health-integration
description: Use when reviewing the standalone OTLP integration scenario's access-log masking and runtime collector health-transition coverage.
knowledge_type: regression-guards
---

# OTLP integration discriminates masking and runtime health transitions

## Conclusion and scope

The standalone OTLP integration scenario must distinguish the configured masked
path from the raw request path in both access-log projections: the JSON body at
`request_info.path` and the typed OTLP `url.path` attribute. It must also exercise
the published `LinkSys/Otlp` generation across a collector outage and recovery
without restarting the Gateway, observing both transitions through the canonical
`edgion_link_sys_health` metric.

This is an integration regression boundary. Production masking remains owned by
`RequestInfo` serialization and `OtlpLogSender::emit_record`; production health
remains owned by `OtlpLinkClient` and its background endpoint probe.

## Rationale and evidence

`edgion-tests/integration/scripts/integration/link_sys/otlp_test_assertions.py`
configures an explicit replacement, sends a recognizable request path, and
requires both projections to equal the replacement while rejecting the raw path
from the complete OTLP record. The same isolated runner stops and starts its
active collector while the Gateway stays alive, using bounded `wait_phase`
polling for unhealthy and recovered metric states.

The canonical projection contract is documented in
`skills/03-coding/observability/00-access-log.md`. The probe-driven OTLP health
contract is recorded in
`skills/04-review/link-sys/intentional-design/healthy-semantics-intentional-non-flips.md`.

## Alternatives and limits

A non-empty `url.path` assertion cannot detect disclosure of the raw path or a
disagreement between the body and typed attribute. Startup with an absent
collector followed by initial recovery does not cover loss of a previously
healthy endpoint. Fixed sleeps are not readiness evidence; the scenario uses
deadline-bounded state polling instead.

A deleted provider may leave its last gauge series until recorder eviction.
When the scenario recreates the same LinkSys key, neither that stale gauge nor a
`direction="recovered"` flip proves publication: client initialization emits the
flip before the persistent runner swaps the client into its published slot. The
scenario retries one identified request within a deadline until the collector
receives that trace, then uses a different single request to verify settled
delivery.

The health check proves TCP endpoint reachability and published health-state
refresh. It does not claim OTLP export acknowledgement, TLS negotiation health,
or collector pipeline correctness beyond the separately identified delivery
record. Before the first generation publishes, the persistent runner exists but
the link has no health gauge series; that startup state must not be misreported
as a published zero-valued gauge.

## Verification and re-evaluation

The dedicated runner is
`edgion-tests/integration/scripts/integration/link_sys/run_otlp_test.sh`. Revisit
this guard if access-log path ownership, the OTLP record projection, the
published LinkSys health observation, or the isolated collector topology changes.

On 2026-09-19 the dedicated runner passed with the explicit replacement and raw
path exclusion, same-process collector degradation and recovery, fallback
delivery while unhealthy, post-recovery OTLP delivery, and identified shutdown
batch delivery.

## Resolution references

- `otlp-integration-masking-health-refresh` — fixed as an integration test gap;
  the historical unconditional `healthy() == true` limitation had already been
  removed before this correction.
