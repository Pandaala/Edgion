---
id: ARCH-347
status: confirmed
kind: behavior-defect
priority: P2
partition: PART-459
analyst: /root/analyze_conversion
baseline: 6758e69fabe58256a1cb96baee1908fd7c0f7d4f469298a812003685bd12b645
---
# Require endpoint evidence in the AllEndpointStatus path-override case

The registered `path_override` integration case can pass when endpoint requests ignore the configured path. Its success predicate observes the diagnostic response envelope rather than the endpoint operation the case promises to verify. This is a test behavior defect, not a claim that current production path selection is broken.

In `all_endpoint_status.rs:688-736`, the case sends GET `/anything` to the basic fixture, requires outer HTTP 200, parses JSON and passes whenever a `summary` field exists. Both local and Kubernetes plugin fixtures configure `pathOverride: /health`; their route points to `edgion-test/test-http:30001`. The actual server's `/health` handler returns the fixed body `OK`. Its wildcard handler also returns HTTP 200, but echoes `Path: /anything` and other request details instead.

Production `fan_out_requests` selects the override at `plugin.rs:503` and records each endpoint's status/body. The final aggregation deliberately returns HTTP 200 even when probes fail. A regression selecting `original_path` instead therefore sends `/anything` to a healthy fixture endpoint, obtains a successful echo response and still returns an aggregate containing `summary`. The current case reports “pathOverride working” without distinguishing that result from `/health`. Requiring only a positive success count would also miss this concrete regression because the wildcard endpoint succeeds. No mutation experiment or observed incident is claimed.

Make this existing case inspect its endpoint results: require at least one selected fixture endpoint and assert the expected successful status, null error and exact `OK` body for those healthy results. Keep the outer status/JSON checks and use the already parsed response; introduce no provider observation, extra request or production logging. Missing endpoints, wrong-path echo bodies, timeouts and probe errors must fail this positive case. Preserve the separate zero-endpoint negative test and every other business assertion, rate gate, deadline and topology boundary.

The existing AllEndpointStatusTls scenario provides an endpoint-result assertion pattern, but its configured and inbound paths are both `/echo`; it does not discriminate this override precedence. An alternative dedicated path-echo fixture would work but is unnecessary because `/health` already has distinct output. Historical “10/10 passed” in the TLS review guard records execution, not this semantic guarantee. ARCH064 concerns malformed method admission; restoration, status-projection and metric-observation findings have different owners.

Future verification should reject a valid aggregate carrying the wildcard echo body despite endpoint status 200, reject empty/failed results, and retain the healthy `/health` pass in local and Kubernetes execution. No source/test changes or execution occurred; no new transport, body owner, framework or performance claim is proposed.
