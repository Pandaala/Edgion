---
name: etcd-configuration-and-message-boundaries
description: Review Etcd configuration enforcement, early message limits and connection admission.
knowledge_type: regression-guards
---

# Etcd configuration and message boundaries

## Conclusion and scope

Etcd configuration is validated in Resources and prepared once per Common
provider generation. The selected local SDK enforces message policy across all
eight services, bootstrap authentication, refresh, maintenance and election.
Field presence or post-buffer length checks do not establish enforcement.

## Rationale and evidence

The authority is the approved support design and the shared mechanisms in
`skills/00-design-rules.md`. Resources `resources/link_sys/etcd.rs` owns field
validation; Common `link_sys_core/providers/etcd/policy.rs` owns preparation.
The local SDK under `third_party/etcd-client` checks exact protobuf size before
encoding and incoming gRPC envelope length before Tonic payload accumulation.
Borrowed arguments are preflighted before copies; namespace range checks use
the final effective range, not an option overwritten by prefix/all/from-key.

Physical connections perform configured authentication and pass Status version/cluster checks before
publication, including reconnects. Dialing uses canonical egress and TLS.
Discovery retains the last valid set and cannot downgrade HTTPS. Retirement
cancels all generation drivers. Election omits unary request timeout while
retaining message, admission and retirement policy.

Common emits typed observations; product binaries own logging and metrics.
Gateway completion counters use bounded labels and canonical registry policy,
without RPC tracing. Retirement is linearized against completion callbacks.
Weak metric ownership does not extend generation lifetime or refresh retired
series indefinitely.

## Alternatives and limits

The former fields were accepted without consumers (B0084). The correction adds
real transport/runtime consumers, removes duplicate timeout.keepAlive, and
rejects arbitrary Etcd metric labels and incomplete policy combinations.
Bounds are per message and transport window, not an aggregate RSS guarantee.
Already accepted messages are not retroactively tightened on reload.
Process-lifetime cardinality budgets remain the canonical Gateway policy.

## Verification and re-evaluation

On 2026-10-05 independent SDK verification passed 26 library and 18 socket
tests. Malicious HTTP/2 peers announcing u32::MAX payloads were canceled after
bounded reads, with the same connection usable afterward. Binary stage tests
cover retirement, trace absence, cached counters, cardinality and idle eviction.
Real cluster acceptance passed: LinkSys 90/90 (including dynamic caps, exact
limit counters, independently absent rejected writes and discovered-node
reconnection), Controller election and Gateway HA/reload. Admin validation and
typed/offline ingest tests additionally verify shared validation and rejection
before a fresh runtime can be built. Workspace format/check/clippy and all
project guards passed. Full results are recorded in
`tasks/todo/etcd-linksys-config-support/03-subtasks.md`. Existing dependency
advisory failures in `05-issues.md` still prevent finding closure.

Re-evaluate on SDK/Tonic/Hyper upgrades, new RPC services, compression support,
namespace option changes, admission/discovery changes or observation ownership
changes. Repeat the all-service and real socket gates, not just mocked tests.

## Resolution references

- B0084 / define-etcd-linksys-unused-config-fields: corrected and functionally
  verified; `tasks/todo/define-etcd-linksys-unused-config-fields.md` remains
  active because existing workspace dependency advisories block closure.
