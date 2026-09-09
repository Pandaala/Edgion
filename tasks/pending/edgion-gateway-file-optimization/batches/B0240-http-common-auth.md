# B0240 — HTTP Common Authentication

## Scope

- Pipeline state: deferred; existing D0052/task expanded, verification pending.
- Active anchor: `edgion-gateway`.
- Baseline: `f470e758`.
- Target: `edgion-gateway/src/plugins/http/common/auth_common.rs`.
- Inspect the complete shared HTTP authentication helper, credential/header ownership, failure response semantics, all plugin consumers and tests.
- Root owns Cargo and all writes. Existing integrations, external todos and handoff/WAF paths remain protected. Revalidate after the active batch before promotion.

## Outcome

- Fresh analysis and independent review confirmed that `set_claims_headers` iterates caller HashMaps directly. Aggregate-cap admission and case-insensitive duplicate destination ownership are therefore nondeterministic; sorting in the request path would add hot-path work without rejecting ambiguous configurations.
- JweDecrypt `payloadToHeaders` is a third consumer of the same helper and mapping shape, alongside JWT and OpenID Connect. Array-valued claims also allocate an intermediate `Vec<String>` and joined string before either cap is enforced.
- Decision: defer, complex. The existing D0052 is the sole correct owner because a safe repair requires one construction-time ordered plan, resource-side normalized destination validation, coordinated immutable publication in all three plugins, and shared cap-aware scalar/array materialization. A target-only helper patch or one-consumer repair is incomplete.
- D0052 and its self-contained task now explicitly cover JWE, incremental bounded array formatting, remaining aggregate budget, sensitive destination writes, deterministic construction and exact boundaries across all three consumers. No new issue was created.
- Independent reviewer `b0236_ai_guard_plugin_analysis`: approved defer and the required record/task amendment. It found no other target-local auth response, challenge, credential-hiding, delay, log/redaction or failure-policy change that clears the value gate.
- Existing integration tests remain unchanged and no integration issue is required; resource validation and Gateway unit tests can prove the future plan.
- Verification: shared auth 19/19, JWT 45/45, OpenID Connect 133 passed/8 ignored explicit network fixtures, JWE 11/11, and resource JWT/OIDC/JWE 34/34. Workspace format/check/Clippy (existing warnings), agent docs, SSA, metrics inventory, Pingora source, Gateway no-tracing, English-only, diff and 3,136-file freeze passed.
- Local record commit SHA is backfilled by the next batch; never push.
