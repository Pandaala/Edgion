# B0239 — AllEndpointStatus Transport

## Scope

- Pipeline state: examined-no-change; verification and independent review complete, awaiting local record commit.
- Active anchor: `edgion-gateway`.
- Baseline: `e0e6c1a3`.
- Target: `edgion-gateway/src/plugins/http/all_endpoint_status/transport.rs`.
- Inspect the complete transport helper, outbound exchange bounds, endpoint-result semantics, runtime callers and tests. The existing AllEndpointStatus module/plugin files remain terminal unless evidence proves exact supporting scope and requires reopening.
- Root owns Cargo and all writes. Existing integrations, external todos and handoff/WAF paths remain protected. Revalidate after the active batch before promotion.

## Analysis Outcome

- Decision: examined-no-change candidate. At `e0e6c1a3`, target blob `2ebce3a04e07fe7a08523aeea2a50f2d139ae83a`; revalidate it and the plugin caller before promotion.
- Each request freezes discovery and BackendTLSPolicy identity, resolves ready service ports, rechecks generation and unsafe addresses near dial, prepares verified TLS once per service group, and builds endpoint-pinned canonical outbound clients with proxy disabled, redirects denied and no idle pooling across same-authority endpoints.
- Request-level and wall deadlines, concurrency three, at most fifty endpoints and a 16 KiB streaming body cap per endpoint bound the diagnostic exchange. Errors are static classifications; no dependency address, payload, Secret or response detail enters logs or client errors.
- Sharing/persisting clients would undermine endpoint isolation and complicate discovery/policy/identity invalidation. The two address checks cover different phases, and explicit default TLS roots intentionally avoid unrelated LinkSys identities. These are established defense-in-depth, not duplicate churn.
- HTTP/2 response-header list bounds remain owned by the existing cross-client task. No transport-local implementation, duplicate issue, knowledge edit or integration change is justified.
- Exact mutation scope is empty. Existing unit and frozen integration sources exercise pinning, unsafe addresses, deadlines, caps, aggregation and dynamic TLS. Historical runs are not fresh verification; root will run focused current tests after independent review.

## Outcome

- Freshness at `f470e758`: target blob remains `2ebce3a04e07fe7a08523aeea2a50f2d139ae83a`; plugin caller remains `c92c10a605b453b397e74d340a9370d79f528b4e`. B0238 changed neither file nor any assessed transport invariant.
- Independent reviewer `b0236_ai_guard_plugin_analysis`: LGTM/examined-no-change. It independently verified endpoint/client lifetime, every address-validation phase, TLS identity precedence, pinned resolution, proxy/redirect/pooling policy, time/body/fan-out bounds, static errors, diagnostic response ownership and current test sources.
- Sharing clients would weaken same-authority endpoint isolation; removing validation would erase queue/dial-time defense-in-depth; ordinary global TLS defaults would incorrectly admit unrelated LinkSys identity. No micro-edit or target-local HTTP/2 header cap clears the value/unification gates.
- Verification: AllEndpointStatus 18/18; workspace format/check/Clippy (existing warnings), agent docs, SSA, metrics inventory, Pingora source, Gateway no-tracing, English-only, diff and 3,136-file freeze passed. No source, knowledge, issue or integration edit.
- Commit: `9b1340bc`; never pushed by the campaign.
