---
name: deferred-credential-connection-nomination
description: Use when reviewing deferred upstream credential materialization, Connection nominations, or upstream_request_filter local-failure ownership.
knowledge_type: regression-guards
---

# Deferred credentials must reject Connection nomination before materialization

## Required behavior

`pg_upstream_request_filter` is the final owner of deferred credential header
materialization. It must validate every `Connection` field from both header
views before inserting a credential:

- `session.req_header()` is the canonical post-plugin downstream request. A
  late plugin can add a nomination here after `CredentialInjector` has staged
  a credential.
- `upstream_request` is Pingora's attempt-local clone. It can differ across
  retries and may contain a later attempt-local mutation.

Pingora sanitizes the attempt-local clone before invoking this hook. Checking
only that clone is insufficient because sanitization can remove the original
`Connection` field and its nominated credential header. Materializing the
credential afterward would reintroduce a sensitive field that Pingora had
already removed as hop-by-hop.

Reject before materialization when either view contains a case-insensitive
nomination of a deferred credential target or any `Connection` value is not
valid UTF-8. Inspect all field lines and all comma-separated tokens. Unrelated
options, including the ordinary WebSocket `Connection: Upgrade` handshake,
remain valid when they do not nominate a credential target.

## Failure ownership

Nomination and non-UTF-8 rejection records the request diagnostic
`CredentialConnectionConflict`. Expiry continues to record only
`CredentialExpired`. The hook returns the same static-detail,
non-retryable `HTTPStatus(500)` used for local framing, credential application,
and AI dispatch-state preparation failures.

This is local pre-dispatch control flow. It must not be attributed to the
selected backend, consume an AI dispatch, or enter upstream retry/outlier
accounting. Neither the error detail nor any log, metric, diagnostic, or access
log field may contain a credential header name or value.

## Regression triggers

- Credential validation reads only `upstream_request` or only the session
  header.
- Validation moves after deferred credentials are applied.
- Only the first `Connection` field or token is checked.
- Local preparation failures become retryable or use `InternalError`.
- Credential target names or values are added to error detail or observability.
- `CredentialConnectionConflict` is emitted for expiry or unrelated local
  preparation failures.

## Integration coverage

Source task: `add-deferred-credential-connection-nomination-integration`
(campaign pointer `D0107`). This was a real integration-test gap, not an
unfixed production vulnerability: the dual-header guard and hook unit tests
already existed when the task was selected.

`edgion-tests/src/client/suites/edgion_plugins/credential_injector/credential_injector.rs`
adds `late_connection_nomination_fails_before_backend_receipt`, configured by
`edgion-tests/integration/conf/EdgionPlugins/CredentialInjector/03_connection-nomination.yaml`.
The shared topology runs CredentialInjector followed by ProxyRewrite, which
sets only `Connection` to nominate the staged target with mixed-case spelling.
It exercises the actual Pingora HTTP/1 sanitizer and upstream hook rather than
manually removing headers before calling the hook.

The test checks a generic 500, `CredentialConnectionConflict`, exactly one
selected upstream attempt, and zero backend captures after access-log completion.
A successful capture first proves the Secret generation and receipt observer.
The dedicated single-endpoint Service has all outlier thresholds set to one,
panic degradation disabled, and base/maximum ejection duration both five seconds.
Local rejections must leave it available; a real backend 502 must eject it as
a positive control. Each availability assertion completes within three seconds
so cooldown cannot hide a transient ejection. Entry/exit recovery polling,
capture cleanup, and access-log deletion have bounded deadlines; failures do
not print captured credentials or response bodies.

This scenario covers the local HTTP/1 topology and a static plugin chain.
It does not establish HTTP/2, Kubernetes deployment, non-UTF-8 wire handling,
or dynamic policy/Secret-reload coverage; existing unit and dynamic tests own
those independent boundaries where implemented.

Fresh verification on 2026-09-09:

- `run_integration.sh -r EdgionPlugins -i CredentialInjector`: all eight cases
  passed, including the new case in 5.27 seconds. The runner also passed config
  synchronization, unknown-field, metrics-label, requeue-loop, and cache-residue
  checks. Report: `tmp/integration_testing/testing_20260909_150645/report.log`.
- The actual build used the committed Pingora lock revision
  `518f7274eb9fbd68f3f55e85b8c86a9e4a59e95e`; no dependency was changed.
- `cargo fmt --all`, `cargo check --workspace --all-targets`, and
  `cargo clippy --workspace --all-targets` passed. Clippy reported existing
  warnings outside the added test.
- Agent-doc consistency, SSA-force, metrics inventory, Pingora source policy,
  and diff-whitespace checks passed.
- Independent read-only review: LGTM. Neither the full integration suite nor
  `cargo test --all` was run.

Disposition: the integration gap is closed by regression coverage; the existing
production guard is unchanged. Revisit this evidence when the Pingora sanitizer
ordering, local error ownership, or test topology changes.
