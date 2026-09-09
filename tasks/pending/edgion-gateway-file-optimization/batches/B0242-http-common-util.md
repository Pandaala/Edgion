# B0242 — HTTP Common Utilities

## Scope

- Pipeline state: examined-no-change; verification and independent review complete, awaiting local record commit.
- Active anchor: `edgion-gateway`.
- Baseline: `761ea605`.
- Target: `edgion-gateway/src/plugins/http/common/http_util.rs`.
- Inspect complete shared HTTP utility parsing and formatting, request-path allocation, all callers and tests.
- Root owns Cargo and all writes. Existing integrations, external todos and handoff/WAF paths remain protected. Revalidate after the active batch before promotion.

## Outcome

- Target blob `b80455f3a878d9ecf21acbf818a2cac79e7786e1` at `761ea605`; discovery, JWKS and userinfo callers and the canonical capped-body comparator remain unchanged from analysis.
- Known decoded Content-Length is rejected only above the cap. Missing or dishonest length is checked on each decoded stream chunk before copying, exact size succeeds, retained bytes stay within the cap, and transient ownership is bounded by retained bytes plus one reqwest chunk and the parsed value.
- Reusing the truncating `stream_body_capped` helper would copy the remaining allowed prefix of an offending chunk before reporting truncation, adding failure-path work solely to remove a clear strict loop. Generic bounded `DeserializeOwned` parsing after collection remains the appropriate shape.
- Dynamic reqwest/serde error strings can carry provider detail, but the existing self-contained P1 OIDC redaction task/D0058 explicitly owns a typed, atomic producer/consumer repair across all endpoints and rejects partial fixes. No amendment or duplicate task is needed.
- Independent reviewer `b0236_ai_guard_plugin_analysis`: LGTM/examined-no-change. It found no target-local cap, allocation, parsing, status, content-type, log/metric or schema change that clears the value gate.
- Verification: helper 4/4 and OpenID Connect 133 passed/8 ignored explicit network fixtures. Workspace format/check/Clippy (existing warnings), agent docs, SSA, metrics inventory, Pingora source, Gateway no-tracing, English-only, diff and 3,136-file freeze passed. No source, knowledge, issue or integration edit.
- Commit: `cf359498`; never pushed by the campaign.
