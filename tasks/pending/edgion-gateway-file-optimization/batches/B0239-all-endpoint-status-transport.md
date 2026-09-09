# B0239 — AllEndpointStatus Transport

## Scope

- Pipeline state: analysis-ready; queued for independent no-change review after B0238 commits, no mutation authorized.
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
