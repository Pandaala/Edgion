---
name: downstream-handshake-offload-budget-and-cancellation
description: Review the Gateway's optional downstream TLS handshake offload, process thread budget, and timeout cancellation boundary.
knowledge_type: regression-guards
---

# Downstream TLS offload has a process thread budget and cancellation owner

## Conclusion and scope

Gateway process configuration `server.downstream_tls_offload_threadpools` and
`server.downstream_tls_offload_thread_per_pool` must both be omitted or both be
in `1..=4`. Omission disables offload. Each distinct terminating HTTPS or TLS
acceptor owns its own Pingora runtime; the startup builder rejects a total
above 64 threads after listener deduplication and conflict filtering. Plain
HTTP/TCP/UDP, TLS passthrough, upstream TLS, and conf-sync TLS are unaffected.

The locally committed fork revision `7c24c0dd6ea5996af011529f6bae8ae8972f695a`
uses an abort-on-drop task handle, and `Cargo.lock` pins that revision for the
complete fork family. The commit has not yet been pushed to the declared Git
remote, so another machine cannot resolve this lockfile until publication.
A listener timeout or caller cancellation must release an incomplete handshake
task and its socket.
The handler remains responsible for the established connection's existing TLS
callback ordering, connection identity, and single SSL log owner.

## Rationale and evidence

`cli/pingora.rs::create_server_conf` validates the pair before constructing
`ServerConf`. `runtime/server/base.rs::configure_listeners` passes a single
`TlsOffloadBudget` through the already deduplicated physical listener loop.
`listener_builder.rs` reserves capacity only for HTTPS and TLS Terminate, then
configures each `TlsSettings` from the validated `ServerConf`. The two fields
describe a process-local policy, not an `EdgionTls` resource or route option.

Pingora's `TlsSettings::build` creates a separate lazy `OffloadRuntime` for
each acceptor. Without the process budget, a seemingly small pair of pool
values multiplies by the number of listeners. The locally committed fork's
BoringSSL acceptor adopts the cancellation behavior from official upstream commit
`4487f7b2ab50f159e4a2cf4f6a6b813f61bb6e19`.

## Alternatives and limits

The feature is opt-in because extra scheduling and threads may worsen small
deployments. Offload isolates task execution from the accepting runtime, but
does not reserve CPU, limit the number of outstanding handshakes, or preempt
synchronous cryptographic work. Worker threads start lazily; configuration
validation does not guarantee later thread creation succeeds. Admission or
rate limits remain separate controls for hostile connection churn.

In a local two-core comparison with a `1 x 1` pool, pure new-handshake
throughput decreased while the Gateway used less CPU. Under mixed traffic,
established HTTPS latency improved, but the actual new-handshake rate also
fell. Under a reused-connection-only control, offload was slower. These
results do not establish that offload alone caused the mixed-run latency
change. Operators should compare the setting with their own traffic and CPU
limits.

## Verification and re-evaluation

The fork's `spawned_task_is_aborted_when_handle_is_dropped`,
`completed_handshake_runs_callback_on_offload_thread`, and
`timed_out_offloaded_handshake_closes_client_connection` tests cover cancellation,
the dedicated callback thread, and a BoringSSL handshake held inside its
certificate callback until the outer wait times out. The fork's
`dropping_runtime_releases_pending_task` and
`independent_pools_release_all_owned_threads` tests check owner release and
independent pool multiplication. Gateway tests cover the configuration pair,
CLI precedence, and aggregate thread budget. The offload-enabled HTTPS,
TLSRoute, mTLS, gRPC TLS, PROXY, SNI, and cipher suites passed locally. The
LastKnownGood dynamic scenario verified valid certificate A to freshly generated
valid certificate B, rejection of an invalid update while B remains served,
and restoration of A. The HTTPS suite also held four incomplete handshakes through
the production listener timeout and observed connection closure.
This 60-second integration case runs only with both `--full-test` and
`EDGION_TEST_DOWNSTREAM_TLS_OFFLOAD=1`; the short CPU-capped comparisons below
were run manually and are not part of the integration runner.
The timeout test keeps the acceptor runtime alive through the client assertion;
temporarily restoring the old bare spawn makes that test fail on client closure,
and restoring abort-on-drop makes it pass.
An offline workspace check with `--locked` consumed the new Git revision without
a Cargo path patch. This local verification does not establish that the declared
remote serves the commit or that performance improves. Publish the fork commit
and rerun the locked checks on a clean checkout before release.

An equal-CPU local comparison used the Linux/arm64 debug binary inside a Docker
container capped at two CPU cores and 4 GiB. Both modes loaded the same four
terminating TLS listeners and warmed each listener to start lazy workers. The
handshake-only and mixed runs used seven closed-loop clients creating fresh TLS
connections to one HTTPS listener. Mixed and reused-only runs also kept one
HTTPS, one plaintext HTTP, and one SSE client active. The driver counted actual
connection reuse; over 99% of requests in each stream reused a live socket.
All requests and handshakes succeeded. These short single runs are diagnostic,
not capacity planning or a statistical performance claim. Shared-container CPU
limits and closed-loop arrivals mean the actual handshake rate differed by
mode, so the mixed-run latency difference is not an isolated causal effect:

| Workload and measure | Off | On (`1 x 1`) |
| --- | ---: | ---: |
| Mixed, new handshakes/s (15 s) | 493 | 413 |
| Mixed, reused HTTPS p95/p99 (ms) | 56.03 / 61.75 | 1.04 / 3.06 |
| Mixed, reused HTTP p95/p99 (ms) | 2.57 / 45.98 | 0.90 / 1.68 |
| Mixed, reused SSE p95/p99 (ms) | 103.39 / 149.18 | 78.27 / 82.92 |
| Mixed, Gateway CPU cores average | 1.497 | 1.550 |
| Mixed, Gateway peak RSS (KiB) | 134872 | 136952 |
| Mixed, Gateway peak threads | 25 | 29 |
| Handshake only, new handshakes/s (10 s) | 917 | 673 |
| Handshake only, Gateway CPU cores average | 1.632 | 1.091 |
| Handshake only, Gateway peak RSS (KiB) | 127760 | 129496 |
| Handshake only, Gateway peak threads | 25 | 29 |
| Reused only, HTTPS p95/p99 (ms) | 0.73 / 0.93 | 3.02 / 4.39 |
| Reused only, HTTP p95/p99 (ms) | 0.68 / 0.86 | 2.84 / 3.89 |
| Reused only, SSE p95/p99 (ms) | 52.77 / 53.20 | 56.81 / 62.65 |
| Reused only, Gateway CPU cores average | 1.311 | 1.399 |
| Reused only, Gateway peak RSS (KiB) | 135176 | 134876 |
| Reused only, Gateway peak threads | 25 | 29 |
| Graceful shutdown after mixed run (s) | 45 | 45 |

The standalone timeout test above covers cancellation under incomplete
handshakes; both benchmark runs completed shutdown in 45 seconds with the same
Gateway shutdown settings. The `1 x 1` policy limits each listener's handshake work
to one worker, which can reduce new-handshake throughput. The Docker CPU quota
includes the traffic client, Controller, and test server; Gateway CPU is sampled
separately.

Re-evaluate if Pingora changes acceptor ownership or offload scheduling, if
listener deduplication changes, or if Gateway adds another terminating TLS
listener type. A change in TLS callback scheduling also requires rechecking
the one-entry SSL log and ex-data contracts.

## Resolution references

- P090-001, `tasks/pingora-0.9.0/001-downstream-tls-handshake-offload.md`:
  optional capability implemented with a bounded process policy and the
  cancellation prerequisite in the local lockfile; the equal-CPU local
  comparison is recorded above. Remote publication and clean-checkout
  verification are still pending.
