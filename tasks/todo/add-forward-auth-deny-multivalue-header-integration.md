# Add ForwardAuth Denial Multivalue Header Integration Coverage

## Meta

- Status: blocked
- Priority: P2
- Category: deferred integration coverage

## Scope and evidence

ForwardAuth copies authentication-service denial headers selected by
`decision.clientHeaders` to the downstream response. Repeated `WWW-Authenticate`
challenges and separate `Set-Cookie` fields must survive; they must not be reduced
to one value or comma-joined. Before this task, the integration assertion read a
single challenge and did not establish repeated-field fidelity.

The original first-value-only production claim was superseded by B0290. Current
`ForwardAuth::send_error_response` uses `get_all`, first insert then append, and
case-insensitive configured-name deduplication; its inline mock test checks those
semantics. This task addresses the remaining real HTTP integration gap.

## Acceptance

- Use the shared integration topology and a real auth denial returning two distinct
  challenges and two separate cookies, including a cookie update or clearance.
- Assert all repeated response fields and per-name order at the downstream client
  using a multivalue-aware API, not a single-value lookup.
- Verify duplicate or case-equivalent configured client header names do not multiply
  the values and unselected auth headers are not forwarded.
- Preserve denial status/body behavior and success-side upstream identity semantics.
- Keep synchronization deterministic and avoid credential-bearing logs.
- Preserve production behavior. The user explicitly authorized this standalone
  test-only work and its verification on 2026-09-11, outside the earlier campaign.

## Implementation and verification

- Added a dedicated auth-server endpoint, paired local/K8s plugin and route
  fixtures, and `ForwardAuthTestSuite::test_deny_multivalue_headers` using exact
  `get_all` byte sequences. Existing success cases remain part of the suite.
- Updated the existing [repeated-header regression guard](../../skills/04-review/forward-auth/regression-guards/deny-response-repeated-headers-preserved.md).
- Canonical preparation built the current binaries and helper executables. The
  initial run stopped on macOS aliases unnecessary for this suite; the runner now
  exempts only an explicitly selected ForwardAuth suite, whose endpoints all use
  `127.0.0.1`. Full-run alias checks remain intact.
- `run_integration.sh --no-prepare -r EdgionPlugins -i ForwardAuth` passed all 26
  cases. The report at `tmp/integration_testing/testing_20260911_225726/report.log`
  is PASS, including final metrics, unknown-field, requeue, and cache-residue gates.
- Workspace format/check/Clippy, agent-doc consistency, SSA force, metrics inventory,
  Pingora source, English-only, Bash syntax/portability, and shell-helper checks
  passed. Clippy has existing unrelated warnings; live K8s and full unit tests
  were not run. Independent read-only code and knowledge review returned LGTM,
  including the scoped runner correction.
- Closure is currently blocked by the pre-existing K8s inventory parity failure:
  `LinkSysDisabledTls`, `PluginPolicyDynamic`, and `RateLimitLocalOverflowDynamic`
  lack K8s registrations. At the initial failure all five inventory inputs matched
  HEAD; the later runner change only narrows the loopback prerequisite and changes
  no inventory entries. Re-running parity still reports those same three gaps;
  the parity script's own regression test passes. Do not delete this task while
  the required check remains unsuccessful. Resolve the unrelated inventory work
  separately, then re-run parity and the final documentation/link checks before
  closing this task.
