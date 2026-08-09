# Changelog — AI Gateway resilience and routing

## 2026-08-04 — Verification complete (awaiting local commit)

### Implementation (A–F)

Delivered per `01-design.md` / `06-decisions.md`: typed HTTP backends and EBTP targets,
AiProxy `backendSelection` (five algorithms, no backend list), credential pool with Redis
RPM/TPM reservation, credential-first retry rounds, shared half-open outlier lease, attempt-aware
response/accounting/access-log attribution. Cap: seven retries / eight attempts.

### Verification (G)

| Gate | Result |
|------|--------|
| `edgion-resources` `crd_coverage` | 10/10 |
| `edgion-gateway` `ai::` unit | 139/139 |
| `credential_reservation_redis --ignored` | 1/1 |
| Integration `EdgionAIService/Resilience` | 3/3 (429 Priority fallback, SSE no-failover, half-open) |
| Integration `EdgionAIService/CredentialExhaustion` | 2/2 |
| AGENTS.md pre-commit suite | all exit 0 |

### Small follow-ups applied during G

- `test_client` `"all"` path includes `ResilienceTestSuite`.
- `skills/05-testing/references/integration-suite-map.md` adds CredentialExhaustion row.
- Session base: `HANDOFF.md`.

### Known thinner coverage (not blockers)

- Adaptive pickers / circuit `maxParallelRequests` / Throttle·Downgrade: unit or SingleProvider heavy; not duplicated in Resilience suite.
- No dedicated AI-3 hot-reload scenario; SingleProvider DynamicTest secret-rotation remains the AI hot-reload coverage.

### Still open

- Local git commit(s) per `02-implementation.md` commit boundaries — **only when user requests**.
- Do not push.
