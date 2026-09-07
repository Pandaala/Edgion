# D0087 — Add Debug Access Log Wire Integration Coverage

## Meta

- Status: needs-implementation
- Severity: medium
- Discovered in: B0125 Gateway API debug access-log analysis
- Category: integration-testing
- Canonical task: [`tasks/todo/add-debug-access-log-wire-integration-coverage.md`](../../../todo/add-debug-access-log-wire-integration-coverage.md)

## Evidence

The frozen debug access-log integration suite reads the terminal AccessLog Store rather than configuring and asserting `X-Debug-Access-Log`. Another fixture includes the plugin but does not inspect the response header. The Controller-to-Gateway publication, response-wire snapshot, Wrap suppression, and startup-global mode boundary therefore lack cross-process coverage.

## Why deferred

B0125's deterministic unit tests prove the policy seam without racing the process-global sensitivity mode. Wire coverage requires separately started Gateway processes or topologies for Replace and Wrap plus new standalone scenario registration. It must not rewrite the campaign's frozen existing integration tests.

## Integration-test impact

This finding is the dedicated integration issue. Add new standalone coverage for Replace, Wrap, and EdgionPlugins hot reload only after separate authorization; do not modify existing integration files during B0125.
