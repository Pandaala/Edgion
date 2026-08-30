# Integration Suite Collapses on Back-to-Back Runs

| Key | Value |
|-----|-------|
| Created | 2026-08-03 |
| Status | todo |
| Severity | blocks using the integration suite as a repeatable regression net |
| Scope | pre-existing; affects **every** suite, not one |

## Symptom

Running `run_integration.sh` repeatedly without an explicit cleanup between
invocations collapses roughly **3–4 runs in 10**. When it collapses, nearly
every case fails at `0.00s` with a connect error — for example
`TCP connect to 127.0.0.1:31944 failed: Connection refused` (HTTPRoute) or
`error sending request for url (http://127.0.0.1:31180/...)` (EdgionPlugins).

Measured 2026-08-03 on macOS:

| Protocol | Result |
|---|---|
| `HTTPRoute/Transport` ×10, back-to-back | 6 meaningful runs (70–71 of 72), 3 collapsed, 1 produced no summary |
| `EdgionPlugins/ExtProc` ×5, back-to-back | **5 of 5 collapsed** |
| `EdgionPlugins/ExtProc` ×1, after `kill_all.sh` | all green |
| `HTTPRoute/Transport` ×10, `kill_all.sh` between each | **10 of 10 stable**, zero flakes |

The control experiment is the important line: the **pre-existing** ExtProc
suite collapses just as readily as the newly added Transport suite, so this is
not a property of the new transport backends (ports 30030/30031).

## Mechanism (likely, from the evidence)

The runner's own teardown prints, every time:

```
[INFO] Stop edgion-gateway (SIGTERM)
[INFO] Escalated edgion-gateway (SIGKILL after 5s)
```

The gateway does not exit on `SIGTERM` within the 5-second grace period and is
always `SIGKILL`ed. A killed process's listening socket can linger briefly; the
next run's gateway then fails to bind, exits, and every case in that run fails
with a connect error while the runner still reports
`✓ allservicealreadyStop`.

Ruled out by measurement at the time of collapse: leftover processes (0) and
socket exhaustion (30 `TIME_WAIT`, far from any limit). No gateway panic
appears in `logs/edgion-gateway.log` — the log simply ends, preceded by the
config-watch stream errors you would expect once the controller goes away.

## Why it matters

A suite that fails wholesale a third of the time cannot be trusted as a
regression net: a real regression is indistinguishable from a collapse without
reading the logs every time, and CI would either flake or teach everyone to
re-run until green — which is how genuine failures get ignored.

## Suggested direction (not prescriptive)

1. Find out why the gateway ignores `SIGTERM` for >5 s and fix the shutdown
   path — that is the root cause, and it is also a production-relevant fact
   (a pod that ignores `SIGTERM` gets `SIGKILL`ed by the kubelet too).
2. Failing that, make the runner's teardown verify rather than assume: poll
   until the listening ports are actually free before declaring
   `allservicealreadyStop`, and have startup fail loudly on a bind error
   instead of leaving the stack half-up.
3. Meanwhile, document the workaround: run
   `edgion-tests/integration/scripts/utils/kill_all.sh` between consecutive
   invocations. With it, 10 consecutive runs were stable.

## Workaround in use today

```bash
for i in $(seq 1 10); do
  ./edgion-tests/integration/scripts/utils/kill_all.sh >/dev/null 2>&1
  ./edgion-tests/integration/scripts/integration/run_integration.sh --no-prepare -r <R> -i <I>
done
```
