# Add ExtProc Unsupported TLS Policy Recovery Integration Coverage

## Meta

- Status: pending
- Priority: P2
- Category: deferred integration coverage

## Scope

ExtProc does not support an outbound validation hostname override or a nonempty
subjectAltNames constraint. Controller advisory errors are not the Gateway activation
boundary. The Gateway client must reject unsupported effective policy, including
inherited policy, before cache/channel use; constructor failure must seal the plugin
resource through the existing invalid-configuration rejection path.

Add a dynamic scenario using the shared integration topology that verifies unsupported
local and inherited policies cannot activate or dial, including failureModeAllow=true.
Then publish a supported policy and verify recovery without restarting the topology.
Use deterministic readiness/phase synchronization and endpoint observations, not fixed
sleeps or credential-bearing logs.

## Acceptance

- Recheck the current client guard, controller capability policy and canonical TLS inheritance semantics.
- Verify invalid generation rejection and absence of processor calls, then valid generation recovery.
- Preserve existing integration scenarios; use an explicitly authorized test-only scope for any necessary harness wiring.
- Do not implement SAN/SNI support, change failure-policy vocabulary or alter unrelated TLS behavior.

This coverage is deferred under the current campaign's integration freeze. Constructor
and in-memory compilation tests are useful but do not establish end-to-end reload/dial behavior.
