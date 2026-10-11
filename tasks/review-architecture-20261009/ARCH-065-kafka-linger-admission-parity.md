---
id: ARCH-065
status: candidate
kind: behavior-defect
priority: P2
partition: PART-216
analyst: /root
baseline: 22f0fb8b6f4353f689ca4ddc172a4dfd98a2d2929dd1709240cbe4a1b3796471
---

# Align Kafka linger admission with its existing producer deadline

## Current ownership and static witness

`KafkaClientConfig` in `edgion-resources/src/resources/link_sys/kafka.rs`
exposes optional `lingerMs: u64` with a 5 ms default. The Kafka branch of
`LinkSys::validate_config` in `link_sys/mod.rs` checks nonempty brokers and
four nonzero queue limits, but never checks linger. Controller
`link_sys_admission_errors` delegates this shared method plus SAN policy;
`update_status` reports `Accepted=True` when those errors are empty.

Gateway `KafkaLinkClient::from_candidate` in
`edgion-gateway/src/link_sys/providers/kafka/client.rs:225-255` independently
checks those limits and additionally rejects any linger duration at or above
its private `PRODUCE_TIMEOUT`, currently 30 seconds. The actual immutable
`KafkaCandidate` and prepared outbound TLS owner do not provide another shared
scalar gate. The Gateway constructor rejects the value before SDK dialing.

An otherwise valid named/namespaced Kafka LinkSys with
`brokers: ["broker.example:9092"]`, no authentication or TLS, default queue
limits and `lingerMs: 30000` therefore has no shared admission error and can
receive Controller `Accepted=True`, yet cannot construct its Gateway client.
This is a source-derived witness, not an executed deployment or reported
incident. The Gateway safety check works; the defect is disagreement about
whether the operator's configuration is executable. First activation cannot
create a client; ordinary hot-update failure may retain the previously accepted
instance under the existing provider publication policy.

## Bounded improvement and alternative

Give `KafkaClientConfig` one pure canonical validation method covering its
existing broker and queue checks together with `linger < produce deadline`.
Make the existing 30 second ceiling a shared policy constant, consumed both by
the validator and Gateway's actual produce timeout. Resources can own this
runtime-independent configuration constraint; it must not acquire a Gateway
or SDK dependency. Delegate the Kafka branch of `LinkSys::validate_config` to
that method. Keep independent Gateway validation before construction using
that same policy rather than deleting its trust-boundary check.

Adding only a second 30000 literal to Controller is a smaller edit, but leaves
the same policy with separately maintained owners. Silently clamping linger or
removing the runtime deadline would change an explicit current constraint.
Neither is needed. Keep omitted linger's 5 ms default and valid zero linger;
zero is invalid for queue capacities, not for batching delay. Values 29999 ms
and below satisfy the current strict inequality; 30000 ms and above do not.
`u64::MAX` must be rejected without overflow or a narrowing conversion.

Expected benefit is deterministic admission consistent with construction and
one owner for future deadline changes. No request latency, memory or throughput
improvement is claimed. This does not unify provider-specific publication,
health, auth, native SDK or fallback lifecycles.

## Required invariants and knowledge

Preserve aggregate pending-record/byte accounting, bounded topic queues,
nonblocking canonical access-log dispatch, topic generation and current
producer cancellation/abort-and-join/drop policy. Keep the existing produced
operation timeout, health/rebuild policy, TLS owner/proof, resolved secret
redaction and immutable generation fingerprint; the latter already hashes the
effective linger value. Keep Controller's serial worker/dependency lifecycle
and existing error/status ordering where multiple invalid fields occur.
No broker-dialing hooks, custom SDK transport, extra queue, request lock,
request allocation, response aggregation or new observability are involved.

Original pages inspected include LinkSys resource architecture, graded design
rules, schema/transport and the Kafka aggregate-queue, SDK-owned broker dialing,
producer-generation shutdown and provider-owned runtime/removal decisions.
Those pages justify the active runtime safety bounds and provider lifecycle;
they do not require accepting a configuration that its constructor rejects.
The shutdown page references historical ISSUE-007 admission work, but a search
of current pending/todo/block tasks found no Kafka linger issue; this finding
claims only the current reproducible field-specific parity gap. Webhook auth
retirement in ARCH051 and generic custom-field arity in ARCH061 are separate.

## Future verification and current limits

Cover omitted, 0, 29999, 30000 and maximal linger values through shared resource
validation and the actual Gateway constructor without SDK/network execution.
Preserve all four zero-capacity diagnostics and empty-broker behavior. Verify
Controller `Accepted=False` for the invalid boundary and both configuration
planes' ordered diagnostics; keep the Gateway's independent rejection and
existing last-good hot-update policy. `resources/link_sys/kafka_test.rs` covers basic serde shape, while
`resources/link_sys/mod_test.rs::kafka_zero_queue_limits_are_invalid` already
covers all four zero-capacity admission diagnostics. The selected Gateway
constructor test covers rejection at the private 30 second boundary; none of
these inspected tests establishes Controller/Gateway linger admission parity.

All ten PART-216 source files and their relevant contracts/consumers were read;
existing tests were inspected as source only. No tests, builds, application or
network operations were executed. Independent verification is pending.
