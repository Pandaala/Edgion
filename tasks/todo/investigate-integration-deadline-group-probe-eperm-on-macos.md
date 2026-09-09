# Investigate Integration Deadline Group Probe EPERM on macOS

## Meta

- Created: 2026-09-09
- Status: pending
- Type: bugfix
- Priority: P2
- Issue: N/A

## Evidence

External commit `bfc2691e` references this follow-up but omitted its record.
Independent diagnosis reproduced the failure on macOS with system Python 3.9:

```bash
python3 edgion-tests/integration/scripts/utils/run_with_deadline.py \
  --timeout-secs 1 --grace-secs 1 -- \
  python3 -c 'import time; time.sleep(30)'
```

After the deadline message, `terminate_group -> process_group_exists` raised
`PermissionError: [Errno 1] Operation not permitted` at `os.killpg(process.pid, 0)`;
exit was 1 rather than timeout 124. The probe catches only ProcessLookupError;
signal_group also does not handle permission errors. No reproduction child or
wrapper remained in the follow-up listing, but this does not prove descendant
cleanup. The helper was unchanged by the external commit and B0203.

The owning `skills/04-review/observability/regression-guards/debug-access-log-wire-and-lifecycle.md`
records the same macOS issue and a historical successful Linux reproduction;
Linux was not rerun in this diagnosis.

## Acceptance Criteria

- Investigate owned subprocess/group lifecycle, reaping and descendants on macOS.
  Do not treat EPERM as absence or suppress genuine cleanup failure without proof;
  never signal unrelated or reused process groups.
- Preserve bounded TERM/grace/KILL, reaping, normal exit codes, child exit 124
  remapping to 125, actual timeout exit 124 and signal forwarding.
- Test normal/direct/descendant timeouts, resistant children, already-absent
  groups, permission failures and forwarded SIGTERM/SIGINT.
- Verify macOS and Linux separately, owned-process cleanup and bounded elapsed
  time. Do not claim the underlying bug is fixed by restoring this task record.
- The existing helper remains frozen during the optimization campaign; runtime
  and test changes require separately authorized implementation.
