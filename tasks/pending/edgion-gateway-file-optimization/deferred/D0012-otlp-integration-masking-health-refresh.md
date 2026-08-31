# D0012 — Refresh OTLP Masking and Health Integration Coverage

## Meta
- Status: needs-implementation
- Severity: medium
- Discovered in: B0051, `edgion-gateway/src/link_sys/providers/otlp/log_sender.rs`
- Category: integration-testing

## Evidence
The frozen OTLP scenario in `edgion-tests/integration/scripts/integration/link_sys/run_otlp_test.sh` only requires `url.path` to be non-empty, so it cannot detect a raw-path disclosure that bypasses the configured access-log masking policy. The scenario also unconditionally passes a documented limitation claiming `OtlpLogSender::healthy()` remains true after initialization, although the sender now has a background endpoint probe that drives health transitions.

## Why deferred
Existing integration tests are intentionally frozen during the Gateway file-quality campaign because they are an independent verification asset. The B0051 production fix is covered by deterministic colocated unit tests; changing the scenario belongs to a dedicated integration-test update and review.

## Required scope
- Keep the existing isolated OTLP collector topology.
- Configure a deterministic sensitive replacement and send a recognizable request path.
- Assert that both the JSON body path and OTLP `url.path` equal the replacement and that the raw path is absent from the record.
- While the Gateway remains running, stop and restore the collector and poll the canonical LinkSys health observation for degraded and recovered states.
- Remove the unconditional stale known-limitation pass.
- Prefer bounded polling over fixed sleeps.

## Exit criteria
The refreshed scenario fails for raw path disclosure and missing health transitions, passes for the current canonical masking and probe behavior, and remains deterministic on supported CI platforms.
