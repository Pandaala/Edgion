# Request Body Hot-Reload Test

This scenario verifies both request-body policy and plugin hot reload without
restarting the Gateway.

The initial phase applies ordered JSON Patch operations that replace the JSON
`version` field with `initial` and remove `discard`. The capped
phase updates `EdgionGatewayConfig.spec.requestBody` to a 16-byte physical cap
and verifies that subsequent requests are rejected with 413. Its protocol
regressions leave an over-limit HTTP/1.1 chunked body unfinished and require the
Gateway to close that connection promptly; the HTTP/2 counterpart requires a
`NO_ERROR` reset for only the unfinished stream and then opens a second stream
on the same connection. The restored phase returns the policy to its defaults
and verifies that the concurrently updated patch plan replaces the version with
`updated-body-version-is-longer`. The assertions cover the complete committed
body in both successful phases.
