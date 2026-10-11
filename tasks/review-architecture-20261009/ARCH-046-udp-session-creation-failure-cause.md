---
id: ARCH-046
status: candidate
kind: behavior-defect
priority: P2
partition: PART-374
analyst: /root/analyze_outbound
independent_verification: confirmed
---

# Preserve the cause of UDP session creation failure

EdgionUdpProxy::get_or_create_session in routes/udp/edgion_udp.rs erases failed
session-permit acquisition (422-426) and family-matched UdpSocket::bind failure
(432-434) into Err(()). Its actual caller handle_client_packet maps every error
to canonical UdpStatus::SessionLimitReached (390-401).

A new client with an empty session map and 10,000 available permits acquires a
permit successfully. If upstream socket creation fails, the helper returns an
error and drops its local permit, restoring availability. No session or receiver
exists, yet the canonical UDP failure entry reports SessionLimitReached. There
is no second production diagnostic restoring the distinction.

The locked Tokio 1.53.1 bind implementation propagates Mio bind and PollEvented
registration failures independently of Edgion admission. Host socket resources,
ephemeral ports or family support can fail while permits remain available.
This is a source witness, not an executed OS failure or observed incident.
Operators and log aggregation receive the wrong capacity signal and can pursue
the wrong limit. The bounded change concerns cause preservation, not new logs,
metrics, tracing, detailed OS messages, retry or traffic redistribution.

Retain the helper and caller but return a bounded typed cause. The existing
UdpStatus can represent admission failure as SessionLimitReached and socket
creation as a fixed category such as UpstreamSocketError. Alternatively use a
small private session-creation enum. Feed the status to the existing
UdpLogEntry::failure path. UpstreamSendError would obscure the stage: no session
or send exists yet. Do not include io::Error strings, addresses, socket settings
or other dynamic dependency details in the category. Preserve redaction and the
existing access-log shape; no general factory/framework is needed.

Preserve established-session fast return, the 10,000-permit bound, second lookup
after failed acquisition, family-matched binding, occupied-entry same-peer
handling, session-owned permits, receiver ownership, identity-aware retirement,
pinned-source filtering and process cancellation. Both failures still drop the
attempted datagram. Do not revoke sessions, change expiry or listener lifecycle,
redistribute weighted failure shares, or add per-packet allocation/locks.

The current test_session_creation_and_limit only constructs a proxy and checks
zero initial sessions; it never calls creation or exercises either failure.
Retain actual cancellation/retirement/permit tests. Extend fixed status
classification coverage and distinguish admission/socket outcomes and permit
release. Use private unit access and a bounded classification seam where useful;
do not add production socket factories, allocation, public visibility or network
fixtures merely for mocks. Future verification must preserve actual caller
wiring as well as the error classification.

Original session-bind-address-family, udp-session-generation-retirement,
udp-process-shutdown and canonical access-log rules were personally inspected
against actual callers/tests. They govern socket family/lifetime/log ownership;
no retrieved intentional decision requires conflating bind and admission errors.
Active affinity, inactivity expiry, best-effort logs, startup-only ports and
cooperative shutdown remain accepted scoped boundaries. Existing ARCH causes
are different. Exact primary hash and full six-file/current support scopes are
in REPORT-PART-374.json. Independent verification pending.

Static read-only analysis only: no production/test edits, builds, tests, network,
Git operations, measurements or application execution. No throughput, RSS,
exploit, immediate-revocation or durable-delivery claim.
