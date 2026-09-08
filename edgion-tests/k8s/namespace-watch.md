# Standalone Kubernetes namespace watch regression

Run against an explicitly selected real Kubernetes context:

```bash
python3 edgion-tests/k8s/scripts/test_namespace_watch.py --context <context>
```

The runner builds `edgion-controller` before creating cluster objects. Use
`--no-build --binary <path>` only to test an already-built binary. Prerequisites
are Python 3 (standard library only), kubectl, Bash, OpenSSL, and a reachable
cluster with a Ready node. The selected identity needs namespace creation/deletion,
Service CRUD, Namespace LIST/WATCH, and Pod/Lease access in the created namespaces.
It needs no Gateway API or Edgion CRDs.

This is a standalone topology exception: the test must control Controller startup
and inject watch failures. It starts one local Controller at a time, then runs
`leader-only` and `all-serve` sequentially. A local kubectl proxy forwards to the
real API; a second test proxy delays selected LIST requests, returns Kubernetes
WATCH ERROR envelopes containing Status code 410, and closes a selected WATCH
connection. In kube-runtime 2.0.1 a bare HTTP 410 handshake response only retries
WATCH, so the test uses the protocol's explicit expiration event. All LIST bodies
and resource object events come from the real API; only error envelopes are injected.
Real fixture CRUD bypasses the fault proxy. No Gateway or HTTP backend is started.

Each run owns three randomly named namespaces: A and B are watched, C is excluded.
The configuration includes A twice to verify source deduplication. Service is the
target of assertions; the existing mandatory EndpointSlice watcher also runs even
when listed in `no_watch_kinds`. Namespace remains a separate initial readiness gate.
An unscheduled placeholder Pod supplies the real leader-election Pod identity;
its nonexistent nodeName prevents running a container. Conf-sync uses the shared
test certificate generator and mTLS, with automatic cluster CA provisioning off.

The assertions cover:

- A completes its first LIST, receives WATCH 410, and waits on its second LIST
  while B completes. The initial raw union must contain A's committed snapshot
  and B, while the delayed first Namespace LIST still keeps `/ready` at 503.
- After readiness, A's pending relist commits without losing B or changing
  server identity or the recorded initial-sync duration.
- During a later A relist, A retains its last committed snapshot while B updates.
  Real deletion and same-name recreation in A converge after the LIST is released,
  including deletion of absent objects and replacement by the new UID and port.
- C stays absent from raw identities and the processed conf-sync cache.
- An administrative reload changes server identity and converges to the complete
  resource set again.
- A resourceVersion-guarded replacement of the owned Lease creates real leadership
  loss and reacquisition. Leader-only becomes unready and rebuilds its serving epoch;
  all-serve retains readiness and server identity, including processing a B update.

Assertions inspect Controller raw cache keys, processed `/configserver/Service/list`
objects, readiness, server identity, and the Service initial-sync metric. API reads
alone are not accepted as cache verification. A background observer samples readiness
and server identity throughout runtime changes, pausing only for administrative reload
and the intentional leader-only outage. Sampling cannot prove the absence of outages
shorter than its observation interval. One-worker ownership is checked by lifecycle
tests and source review; live metrics do not expose task identity. This test covers
Controller/API interaction and conf-sync publication, not Gateway HTTP forwarding.

Logs and a JSON report remain under `tmp/integration_testing/namespace_watch_*`.
Cleanup terminates only owned subprocesses and deletes only namespaces whose
recorded UID and ownership label still match, then verifies their disappearance.
It never changes kubectl's current context, installs CRDs, or edits existing
deployments. Proxy diagnostics omit headers, tokens, and resource bodies.
