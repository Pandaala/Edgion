# MCP backend dynamic lifecycle

`McpBackendLifecycle` reuses the integration harness's single Controller and Gateway topology.
Every transition is observed through `wait_phase`; the scenario starts no services and uses no
fixed sleep. It covers Service and external targets, valid-to-invalid-to-valid bearer rotation,
Secret delete/recreate, combined cross-namespace Service and Secret authorization, ReferenceGrant
apply/update/delete/recreate requeues, dependency replacement, an MCP-to-A2A variant switch,
Controller status, Gateway admin redaction, and backend deletion. WP9 has no client-facing MCP
frontend, so sensitive header bytes and snapshot generation are intentionally verified by Gateway
unit tests instead of a fake dynamic request path.

The scenario requires the normal full-integration fixture set: `HTTPRoute/Basic` supplies the
`edgion-test/test-http` Service used by the in-namespace Service target.

These files are also loaded by the static conf loader before the dynamic scenarios run, so
`edgion-test/mcp-lifecycle-tools` is normally already published when the scenario starts. The
scenario therefore waits for the Gateway to withdraw it after the initial cleanup before it
captures the unified runtime-store baseline the phase assertions compare against.
