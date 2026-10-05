---
name: transactional-controller-channel-selection
description: Review Gateway Controller candidate probing, channel publication, TLS reload races, and remote-lap recovery accounting.
knowledge_type: regression-guards
---

# Transactional Controller channel selection

## Conclusion and scope

Gateway conf-sync separates the installed connection from the search for a
replacement. `InstalledChannel` groups address, address-list index, service handle,
and reload generation. Candidate traversal is owned by `EndpointRotator` under the
channel manager's publication lock; it is not evidence that an address is installed.
Certificate metadata now belongs to the same installed snapshot; see
[installed certificate expiry](installed-certificate-expiry.md) for the current
scrape-time reporting contract. The verification below is historical evidence
for channel selection, not fresh verification of that later correction.

Multi-address recovery probes `GetServerInfo` on an on-demand candidate channel
before installing that exact channel. It does not maintain permanent standby
connections or start a second metadata registration. A Controller that cannot
currently publish a `ConfigSyncServer` rejects the probe through its existing
provider gate. This supports leader-only and all-serve serving modes without
duplicating election state in the Gateway.

## Rationale and evidence

The original `conf-sync-rotation-transactional-address-selection` finding identified
two coupled defects: `rotate()` advanced both cursor and lap failures before local
channel construction could fail, and `reload_client_tls()` selected its address
before obtaining the swap lock. An uninstalled candidate could be skipped; a reload
queued during an A-to-B switch could subsequently reinstall A.

The correction uses these boundaries:

- Local TLS/channel preparation does not consume a candidate. Actual failed remote
  attempts advance the search while leaving installed clients untouched. Traversal
  eventually revisits the original address, so its recovery remains discoverable.
  Every immutable address is URI-syntax-checked at startup to avoid indefinite
  local retries behind a permanently malformed later candidate.
- Failed-lap pauses are consumed once per completed lap and capped with jitter.
  Successful candidate probes do not erase failure history; a current session's
  first valid metadata event does. Thus info-success/meta-failure cannot bypass
  backoff. Local preparation retries have an independent bounded pause.
- A candidate carries both installed-snapshot identity and selection revision.
  Commit and failure accounting reject stale or duplicate results, including a
  probe finishing after TLS reload. The complete unary response has a deadline;
  the network round-trip holds no swap lock.
- Publication updates cache clients before installing the new snapshot and global
  token, then cancels the old generation. A finite spawned publisher finishes even
  if its awaiting caller is cancelled; task failure is not reported as success.
- TLS reload selects the current address within the publication boundary and
  installs validated local material independently of remote availability.
- Installing a newly probed channel after a multi-address wrap bumps generation
  even if its address equals the old address. Single-address endpoint rotation
  remains a channel/generation no-op.

Owning code: `edgion-gateway/src/conf_sync/conf_client/{grpc_client,rotation,mod}.rs`
and the narrow bootstrap caller in `edgion-gateway/src/cli/mod.rs`. Controller
service eligibility is implemented by `ConfigSyncGrpcServer::get_server()` and
the provider/lifecycle; no new health RPC or wire fields are required.

Authority: [graded design rules](../../../00-design-rules.md), especially concurrency
and no locks across network round-trips; [Gateway-side architecture](../../../01-architecture/03-controller-gateway-link/02-gateway-side.md).

## Alternatives and limits

A successful probe is point-in-time serving evidence, not a leadership lease or a
guarantee that subsequent List/Watch calls succeed. The server remains responsible
for serving eligibility, and runtime recovery remains necessary. Dedicated health
RPCs do not eliminate that race. `WatchServerMeta` is unsuitable as a disposable
probe because it registers the Gateway and affects instance counts.

This is channel-publication consistency, not atomic configuration publication
across resource kinds. Already applied callbacks are not retracted. Existing
[per-kind relist backoff](../../concurrency/intentional-design/relist-backoff-on-server-switch-intentional.md)
and the accepted [certificate metadata signature](../intentional-design/conf-sync-cert-watch-mtime-length-signature.md)
remain separate mechanisms and are unchanged.

The historical [watch endpoint timeout evidence](../../concurrency/review-evidence/conf-sync-watch-endpoint-timeout.md)
concerns stream response headers, not a complete unary response body; it cannot
replace the candidate probe's complete-response deadline.

## Verification and re-evaluation

Fresh verification on 2026-09-07:

- `cargo check -p edgion-gateway --lib`: passed at the implementation checkpoint.
- `cargo test -p edgion-gateway --lib conf_sync::conf_client:: -- --test-threads=1`:
  26 passed after the final test corrections. This includes local TLS failure,
  queued TLS reload after candidate commit, duplicate/stale commit rejection,
  cancellation while a real cache-client lock blocks publication, same-address
  generation replacement, single-address stability, and failed-lap accounting.
- `cargo test -p edgion-gateway --test conf_sync_failover`: passed again after the
  test-lock lint correction. One sequential wire test covers retained installation
  during candidate probing, non-serving rejection, reuse of the same TCP channel,
  runtime failover, a fresh meta session on the recovered original address, and
  an HTTP/2 response whose successful headers are followed by a stalled unary body.
- `cargo fmt --all`, `cargo check --workspace --all-targets`, and
  `cargo clippy --workspace --all-targets`: exit 0. Clippy reports pre-existing
  warnings; diagnostics on this change's added/modified Gateway lines were checked
  and none remain. The unchanged `start_watch_kinds` match still has its existing
  `manual_let_else` warning.
- Agent-doc validation, SSA force, metrics inventory, Pingora source policy,
  Gateway tracing boundary, and `git diff --check`: passed.
- Separate independent plan and code reviews approved the design and implementation;
  the code reviewer rechecked the final lint corrections. No commit or push.

The first queued-TLS unit run needed the test process's Rustls crypto provider to
be initialized; the corrected test passed. Production already initializes its
provider. Network tests simulate Controller serving eligibility, not real leader
election, and do not establish real remote mTLS or per-kind List/Watch coverage.
The TLS unit verifies local material reconstruction and serialized address choice.
Existing integration-test files were not modified. The full workspace test suite
was not run.

Re-evaluate when Controller serving eligibility changes, parallel recovery is
introduced, cache-client publication gains new waits or failure points, generation
ownership changes, or a new probe method gains registration/other side effects.

## Resolution references

- `conf-sync-rotation-transactional-address-selection` — fixed, 2026-09-07,
  following the user's candidate-probing decision.
- `CONF-SYNC-CLIENT-001` — original campaign investigation deferred this repair;
  that investigation's historical checks are not verification of this correction.
