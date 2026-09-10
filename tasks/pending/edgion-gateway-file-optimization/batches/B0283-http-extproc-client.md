# B0283 — HTTP ExtProc Client

- Pipeline state: analysis-ready; read-only candidate, not approved for implementation.
- Active anchor: `edgion-gateway`; baseline `9b787978`.
- Target: `edgion-gateway/src/plugins/http/ext_proc/client.rs`.
- Analyst: `b0283_extproc_client_analysis`; client-owned transport, lifecycle, shared mechanisms and coverage/value analysis assigned.
- B0282 owns the serial plan-first lane. Candidate must be refreshed after that batch commits, before implementation or independent review.
- All integrations and user handoff/WAF paths protected; no technical mutation authorized.
- Analysis interrupted by provider usage limit before a final report. Partial unsupported TLS-policy enforcement candidate is unapproved; resume the original analyst and revalidate current HEAD before any promotion.
- Capacity restored; analyst completed change report at 9b787978, target blob `807f1a7ff143420c41c990813df33230c792da6e`. Reject unsupported effective hostname/nonempty SAN constraints after canonical TLS resolution before cache/channel use; do not implement deferred SAN/SNI support.
- Evidence: controller capability guard is advisory, shared validation lacks the guard, and gateway construction currently ignores these constraints. Constructor errors already reach sealed fail-closed resource activation. Preserve CA-only and empty/absent SAN behavior, inheritance and local block replacement semantics.
- Proposed scope: client.rs plus owning ExtProc knowledge index and one regression guard. Security/transport-boundary change requires plan-first before implementation. Nonnetwork constructor/explicit-default and sealed-activation tests; no global TLS mutation or live-server tests.
- Separate findings pending promotion/review: dynamic unsupported-policy recovery integration gap; obsolete exchange_all/rollback knowledge; live gRPC tests in unit tier. D0148 remains separately owned. No task creation or next-batch review before B0282 commits.
