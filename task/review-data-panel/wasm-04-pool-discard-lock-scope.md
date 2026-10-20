# Wasm pool discard lock scope

Status: deferred lock-scope and admission-order decision. No runtime change.
Source baseline: d8682730d.

## Source evidence

In `edgion-gateway/src/plugins/http/wasm/vm_manager.rs`, `VmLease::drop` first
releases callback-frame payloads for a healthy lease with no open context. It
then takes the entry's pool mutex, checks retirement/local pool capacity, and
attempts global idle admission. When no idle reservation exists, `drop(vm)` runs
inside this mutex before discard telemetry and the pool gauge update.

`PooledVm` field order destroys its Store before releasing the external guest
memory charge, actual-instance permit and idle reservation. These guards must
outlive the resources they account. Destruction therefore includes more than
removing one vector element, although its actual cost has not been measured.

`acquire_inner` takes the same entry mutex to pop an idle VM, then attempts fresh
creation after a miss. `new_vm` uses nonwaiting process-instance admission and
guest memory is also budgeted. Moving rejected-VM destruction after unlocking can
let an acquisition observe an empty pool while the discarded VM still retains
its instance permit and guest charges. At saturation this may expose a rejection
that previously waited behind that entry's destruction. Releasing guards before
the Store to avoid it would violate the resource accounting boundary.

This serialization is entry-local, not a global admission guarantee. Other entries
already race for process budgets. `ModuleEntry::retire` already takes the idle
vector under lock and drops it outside, and poisoned/open-context leases take a
different discard path. These paths must not be misrepresented as all holding the
pool mutex during destruction.

## Decision alternatives

- Keep the current scope unless measured discard duration and contention justify
  a change. Existing code can be the best outcome for the actual workload.
- Move only the rejected healthy VM outside the critical section, retaining the
  retirement/idle-admission decision and pool gauge update under the lock. Review
  the observable ordering of resource release, acquisitions and discard telemetry.
- If saturation behavior requires a different admission policy, design that change
  separately. Do not add a waiting queue, background destruction task, global pool
  registry or release permits early as an incidental optimization.

Measure Store destruction duration, pool lock wait, successful/offered throughput,
latency and admission rejections together. Cover pool-cap zero, idle-budget zero,
full local pools, process-instance and guest-byte saturation, concurrent checkout,
retirement and overlapping runtime generations. Shorter critical sections alone
do not prove lower end-to-end latency or fewer failures.

## Acceptance for a selected implementation

Preserve serialized retirement/repool decisions and pool gauge updates. No retired,
poisoned or open-context lease may reenter an idle pool. Do not call guest lifecycle
hooks from Drop. Keep callback payload release before retention and preserve the
Store-before-accounting-guards field order on every path, including initialization
failure, normal discard and cancellation. Existing active old-generation leases
remain valid, with charges held until actual destruction.

Exercise concurrent checkout and rejected return under saturated process limits,
retirement racing returns, global idle count/byte rollback, deferred completion,
abandoned contexts and poisoned guests. Existing configured idle-limit and retired
pool tests in `tests/unit/plugins/http/wasm/vm_manager_test.rs` are starting coverage;
new ordering requires a deterministic concurrency test and explicit acceptance.
Document any changed admission timing instead of calling it behavior-neutral.

Any selected implementation requires its own frozen scope, IMPL plan, independent
reviews and project checks. This task makes no runtime change or performance claim.

Relevant original knowledge:
- skills/04-review/wasm/regression-guards/request-execution-lifecycle.md
- skills/04-review/wasm/regression-guards/tick-quiescence-on-zero-reference-eviction.md
- skills/04-review/wasm/regression-guards/wasm-module-cache-key-runtime-config.md
