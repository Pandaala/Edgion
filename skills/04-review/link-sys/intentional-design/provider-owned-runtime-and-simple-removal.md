---
name: provider-owned-runtime-and-simple-removal
description: Use when reviewing unified LinkSys ownership, current lookup, deletion, or proposals for cross-provider teardown barriers.
knowledge_type: intentional-design
finding_id: link-sys-cross-provider-partial-replacement
---

# Provider-owned LinkSys runtime and simple removal

## Decision and scope

The user explicitly requested one namespace/name-to-current-provider runtime
table, provider-owned lifecycle, and simple removal. Type changes and deletion
are rare; temporary unavailability and asynchronous retirement are accepted.
This is an intentional contract, not a claim that dropping a map Arc immediately
destroys every connection or completes every background task.

The source LinkSys configuration snapshot remains separate. It contains no live
clients and does not compete with the unified runtime table's lookup authority.
Typed getters, health inspection, counts, and keys read current entries only.
Same-type updates preserve provider reuse and existing runner handover/retry.

Removal withdraws current lookup and signals provider-owned cancellation. Old
cleanup never removes a newer same-key entry and never replays creation. Type
change removes the old type and creates the new one without waiting for old
cleanup. External caller-held handles may finish or fail under the provider's
existing semantics; no global handle-revocation guarantee is introduced.

## Benefit, cost, and rejected alternatives

The model removes duplicate provider registries, Webhook publication bookkeeping,
deleting/pending maps, and their join-replay coordination. It avoids adding a
second supervisor above each existing persistent provider runner.

The accepted cost is temporary call failure during type change and brief overlap
with retiring work. Instance limits count current registered objects, not every
old transport clone or background task. Capacity-denied desired resources remain
in the source snapshot and are reconsidered when admission capacity changes.

The earlier proposal for stable tombstone owners, retire-before-create,
cross-provider completion barriers, and mandatory joining of all SDK tasks was
explicitly simplified by the user. Do not restore that machinery merely because
ordinary Drop cannot await. Do retain a real cancellation path: a task holding an
Arc must not prevent its own stop signal from ever being sent.

Existing OTLP process-exit cleanup is a separate provider-specific behavior, not
a shared lossless-log contract. See [process exit](process-exit-log-delivery-best-effort.md).
Canonical HTTP/TLS/egress/DNS/template/secret handling and plugin failure policy
are unchanged.

## Evidence and verification boundary

Implementation owners are Gateway link_sys/runtime/store.rs and provider
adapters. Webhook owns its prepared managed client and explicitly aborts its
health task on retirement, including when a call handle retains the client.
Persistent runners receive begin_delete, then perform their existing shutdown.

The motivating issue was a Webhook-to-other-provider partial update that left
an independently registered old Webhook callable. One typed lookup authority
eliminates that stale registry rather than coordinating two publications.

Verification on 2026-09-21: all 274 Gateway LinkSys unit tests and the replacement
fixture/DSL validation test passed. The shared Redis suite passed 71 assertions
and its cache-residue check. `LinkSysProviderReplacement` passed against the
standard shared topology with `base,HTTPRoute/Basic,EdgionPlugins/base` loaded:
Webhook to Redis to Webhook, deletion, recreation, rapid recreation, and a
non-retried ten-second stability window with fresh traffic. A prior attempt
after the Redis runner's final configuration wipe lacked the complete baseline
and returned RouteNotFound; it is not successful lifecycle evidence.

Independent Astra source review approved the current store and scenario under
this contract. These results establish current lookup and bounded convergence
evidence, not completion of every retired SDK task or live coverage of every
provider pair. Historical test results never substitute for fresh verification
after a relevant change.

## Re-evaluation triggers

Revisit if frequent deletion causes measured resource accumulation, requirements
introduce strict simultaneous-connection limits or old-handle revocation, or a
specific provider fails to stop after cancellation. These are distinct from an
unmeasured desire for synchronous complete reclamation on every deletion.
