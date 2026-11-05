---
name: preload-claim-before-owned-inputs
description: Review Wasm response misses for unnecessary owned inputs before the canonical preload claim.
knowledge_type: regression-guards
---

# Claim Wasm preload before copying task inputs

## Conclusion and scope

Wasm response-header and response-body misses consult the configured plugin's
private runtime slot. Engine/runtime availability, publication, retry deadline,
and the atomic preload claim are checked before copying task inputs or spawning
work. Warm access reads the private atomic publication directly; no global map
lookup or runtime key is needed.

URL artifact fetch now validates the actual bound Wasm entry before all three
supported stage caches, private runtime creation, and preload. The canonical
local TLS proof and one captured global snapshot prepare an immutable HTTP
client result owned by RuntimeState. Async module loading borrows that result;
it does not reload defaults or reinterpret detached TLS material. The runtime's
config clone drops consumed TLS carriers while the original resource stays intact.

Identity or inheritance failures reject construction before runtime ownership.
Crypto/client construction failures remain module-load errors, retaining the
existing negative cache and failOpen decision. Absent or disabled custom TLS is
inert: HTTPS still uses native system TLS, ignores disabled verify/CA/client
settings, and does not inherit custom global TLS. Non-URL sources skip fetch TLS
preparation. OCI and guest callout behavior remain outside this change.

## Rationale and evidence

Finding `avoid-wasm-response-inflight-cache-key-allocation` (campaign source
`D0098`) identified a real, low-severity performance defect: response misses
cloned the key before checking whether preload was already claimed. Each chunk
during a cold load or negative-cache window could allocate a discarded String.

The mechanism lives in `edgion-gateway/src/plugins/http/wasm/plugin.rs` (build and
both synchronous response callbacks) and `vm_manager.rs` (runtime preload).
The engine/runtime checks, published-module check, retry deadline, and atomic
preload claim precede creation of task-owned inputs. `PreloadClaimGuard` still
releases the claim on drop, including task cancellation.

This follows the hot-path performance rules in
[the design index](../../../00-design-rules.md#u-gateway-hot-path-performance--bounded-memory).
The [plugin instance isolation guard](wasm-module-cache-key-runtime-config.md)
now replaces runtime-key identity and registration. The original allocation
correction remains relevant as a claim-before-copy rule even though the key
itself has been removed.

## Alternatives and limits

The preload claim remains atomic; the task ownership gate is entered only for
work that won the claim. No response buffering or per-chunk diagnostics are added.
Response fail-open/fail-close handling, body trap behavior, streaming, and
request-owned diagnostics remain unchanged. The private slot is constructed with
the plugin; a successful task still owns its inputs. This rule does not claim
that the entire Wasm body callback allocates zero.
There is no measured throughput improvement or allocation-count benchmark.

## Integration readiness after preparation saturation

Configuration convergence does not await all private Wasm module loads. The
process preparation limit can reject a best-effort preload; that failure uses
the existing 30-second negative cache. A synchronous response-header miss
schedules a later preload but still applies its configured failure mode to the
current response. This is observable even when the request-stage callout and
upstream response both succeeded.

The static Wasm suite first polls both ordinary and advanced routes under one
45-second deadline. Each route must return HTTP 200, the guest-added request
marker in the upstream echo, and the guest-added response marker. Only then do
the existing callout, trap recovery, performance and fresh tick-delta cases run.
Readiness failure fails the suite; it is not a skip or permission to accept 500
in a business assertion. See `edgion-tests/src/client/suites/edgion_plugins/wasm/wasm.rs`.

The ordinary fixture omits its unused JSON configuration (`{}` would be read as
a polling origin by this guest). The advanced response-stage fixture also has no
polling origin because its independent runtime has no callout grant.
Rust SDK 0.2.5 panics on the host's unsupported
dispatch status before returning a Result, so ignoring a rejected call's Result
does not prevent a trap. The request stage retains its authorized polling origin
and both fresh tick assertions. Local and Kubernetes fixtures follow this same
ownership boundary; local execution does not establish Kubernetes acceptance.

The BodyPlugins response-body probe also waits for its independent runtime.
One successful HTTP warmup does not prove readiness: a cold body callback only
schedules preload and preserves streaming, including during the 30-second load
retry backoff. The probe consumes each nonempty HTTP 200 response and polls for
a fresh increase in `edgion_wasm_body_pause_unsupported_total` under one 45-second
deadline. Once ready, a separate request must increase the counter again.
Warmup request, body-read and scrape errors fail immediately; no observed hook
progress before the deadline is a failure. This changes test readiness only,
not production preload, admission, retry or body-streaming semantics.

Local verification on 2026-10-25 passed all 32 BodyPlugins cases both before
and after Controller/Gateway reload. A response-stage-only fixture pointing to
a temporarily absent artifact reproduced the old probe failure (31/32 passed).
Restoring the artifact during the same runtime's negative-cache window let the
updated suite pass after about 30 seconds; a second run on that Gateway passed
immediately. This exercises delayed-load recovery, not a full integration sweep
or live Kubernetes acceptance.

Fresh local verification on 2026-10-16 passed both rounds of
`run_integration.sh --no-prepare -r EdgionPlugins -i Wasm --with-reload`, ten
cases per round. Startup logged preparation-capacity rejection; the first
round recovered through the readiness gate and took 28 seconds, while the
post-reload round took two seconds. Final metrics, unknown-field, requeue-loop
and cache-residue gates passed. This is scoped local verification, not a full
integration sweep or live Kubernetes execution.

## Verification and re-evaluation

Historical `borrowed_preload_inputs_survive_concurrent_misses_and_retry` retained the first
claim while 64 concurrent callers attempt preparation, verifies they are rejected,
and checks caller-input independence, slot identity, claim release, negative-cache
suppression, and retry. It uses no network or timing sleeps. Allocation placement
is established by the borrowed signatures and the copy after the claim; the test
checks lifecycle behavior rather than counting allocator calls.

Historical verification (2026-09-10, before plugin-owned runtime replacement):

- `cargo test -p edgion-gateway --lib preload`: all four tests passed, including
  the new deterministic test, cancellation release, unavailable engine, and
  the existing single-flight/negative-cache test.
- `cargo fmt --all`, `cargo check --workspace --all-targets`, and
  `cargo clippy --workspace --all-targets`: passed. Clippy reported warnings
  outside the changed code; this was not a warning-free workspace run.
- `validate_agent_docs.py`, `check_ssa_force.py`, `check_metrics_inventory.py`,
  and `check_pingora_source_policy.py` under `cicd/checks/`: passed.
- Independent read-only review: LGTM. Existing integration tests were not
  modified or run; no full-workspace test run was performed.

The 2026-09-23 ownership run passed all 226 selected Gateway Wasm tests.
Replacement tests preserve single-flight, negative-cache, claim cancellation,
and unavailable-engine coverage without retaining obsolete key APIs.

Re-evaluate if preload gains another entry point, owned inputs move before the
claim, or cache identity, slot creation, retry, or cancellation semantics change.

The recorded reload-mode run above predates removal of the administrative reload
API. Current two-round recovery runs use `--with-restart`, which restarts only
the Controller and preserves the Gateway process. No new live run is claimed
by this documentation update.

## Resolution references

- `avoid-wasm-response-inflight-cache-key-allocation` / `D0098`: fixed
  2026-09-10; unnecessary key copying on unsuccessful preload attempts removed.
