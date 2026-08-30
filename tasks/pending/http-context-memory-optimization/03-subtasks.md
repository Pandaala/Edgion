# Subtasks

| Stage | Status | Deliverable | Review | Commit |
|------:|--------|-------------|--------|--------|
| -1 | completed | Unified request-owned WAF state | completed; one selection-terminal regression fixed | `e55009ee` |
| 0 | completed | Task plan and layout/allocation baseline | completed; nested measurement and lifecycle findings fixed | `ae423041` |
| 1 | completed | Allocation-free default Context and lazy stage logs | completed; no blocking findings | `f9e377c9` |
| 2a | completed | Shared request-stable GatewayInfo generation | completed; no blocking findings | `9db6d4a2` |
| 2b | completed | Compact HTTP backend selection with generation/index handles | completed; Arc classification and TLS test findings fixed | this commit |
| 2c | pending | Compact gRPC backend selection with request-stable TLS policy | pending | pending |
| 2d | pending | Remove residual AI/direct-endpoint backend copies | pending | pending |
| 3 | pending | Common/TLS/gRPC RequestInfo split | pending | pending |
| 4 | pending | Optional boxed plugin request state | pending | pending |
| 5 | pending | Optional request-body-plugin and response-processing state | pending | pending |
| 6 | pending | Compact parsed parameters and remaining rare state | pending | pending |
| 7 | pending | Dedicated bounded WAF access-log block | pending | pending |
| 8 | pending | Final verification and measured summary | pending | pending |

## Review protocol

For every stage, the reviewer must inspect only the stage diff and report:

- correctness and ownership findings;
- request hot-path allocation or pointer-chasing regressions;
- response/request body streaming violations;
- access-log sensitivity and boundedness issues;
- missing tests or stale task measurements.

The implementation agent fixes material findings, obtains a clean independent
review, and then creates the local commit under the user's existing
authorization. Commits are never pushed by this workflow.
