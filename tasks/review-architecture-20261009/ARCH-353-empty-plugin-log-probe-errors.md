---
id: ARCH-353
status: confirmed
kind: behavior-defect
priority: P2
partition: PART-467
analyst: /root/analyze_outbound
baseline: 11aeb2347657135da6388057bbf90a4f239e5c16b822a8132cc94916aed7718b
---
# Require observed empty plugin logs instead of accepting probe failures

`PluginLogsTestSuite::test_empty_plugin_logs` reports success for every error returned by `send_and_fetch_log`. The helper owns three distinct steps: send the Gateway request, retrieve its terminal record through the AccessLog Store Admin API, and deserialize that record. Its `Result<AccessLog, String>` carries failure from any step, but this case's `Err(_)` arm labels every failure “no access log stored” and passes. A refused data-plane connection therefore passes this case without sending a request; an Admin transport/authentication failure or incompatible log document also passes without establishing empty plugin logs. This is a source-level false-positive witness, not a reproduced integration incident or a claim that the entire suite passes during a permanent outage.

The absence allowance is justified by a comment claiming simple requests may not be stored. The current producer does not implement that policy. `pg_logging::logging` stores the canonical terminal JSON when integration-testing mode is enabled and the request contains `access_log: test_store`, irrespective of whether plugins ran. This helper supplies that header and a unique canonical W3C traceparent. `AccessLogEntry.stage_logs` is omitted when empty, and this suite's existing serde default correctly converts that omission into an empty vector. Thus a successfully retrieved record already represents the legitimate no-plugin outcome.

The Admin endpoint distinguishes a missing store entry through its response envelope; `AccessLogClient` also returns errors for HTTP, transport and JSON failures. Store TTL and bounded admission can make a record unavailable, but those are failures to obtain evidence, not proof that no plugin executed. The preceding store-status case does not repair the boundary: infrastructure can fail after that check, and it does not witness this request or its correlated terminal record.

Change only the empty-case outcome boundary: retain the successful-record empty-vector assertion and return a failed `TestResult` with the existing helper error on `Err(e)`. Preserve the test identity, request host/path, listener context, trace correlation, retry limits, fixtures and all other assertions. No new production logs, metrics, API, parser framework or client error taxonomy is needed. An alternative allowing explicit not-found would require a documented producer policy and a successful transport witness; current source supplies neither justification for absence nor permission to classify all errors as absence.

Future focused checks should distinguish a valid record with omitted/empty stages, a record with nonempty stages, request failure, Admin failure and malformed record. Preserve terminal-store versus debug-response-header ownership and the accepted development-header limitations. Both fixture trees, actual store/API producer and relevant real unit bodies were inspected. No tests, builds, requests or mutations were performed.
