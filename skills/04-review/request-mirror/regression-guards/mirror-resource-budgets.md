---
name: mirror-resource-budgets
description: Review RequestMirror numeric validation, process admission, retained body ownership, and snapshot streaming.
knowledge_type: regression-guards
---

# RequestMirror resource budgets

## Conclusion and scope

RequestMirror has two admission levels: `MirrorTuning.max_concurrent` limits a
plugin instance, while the gateway-local `request_mirror.max_concurrent`
limits the whole process, including overlapping configuration generations.
Both default to 1024. Routes never instantiate a new process budget.

`MirrorTuning.max_buffered_chunks` defaults to 5; the new
`max_buffered_bytes` defaults to 1MiB and bounds outstanding stream payload
even after it has left the queue. The gateway-local
`request_mirror.max_buffered_bytes` defaults to 64MiB and covers all retained
mirror payload: owned stream chunks and isolated in-memory snapshots. Local
configuration is restart-only; resource tuning remains hot-reloadable.

All operative limits are configurable. Shared safety ceilings are 65536 tasks,
1024 queued chunks, and min(1GiB, the platform Tokio semaphore ceiling) bytes.
They are safety domains, not suggested production settings. Increasing the
process budget explicitly increases the memory allowed to mirror traffic.

## Rationale and evidence

D0066 (`bound-request-mirror-concurrency-and-buffering`, discovered in B0102)
identified unconstrained `usize` configuration reaching `Semaphore::new` and
`mpsc::channel`. Zero-only constructor fallback did not prevent oversized
values from panicking. Per-instance concurrency and chunk counts also failed
to bound aggregate retained bytes.

- `edgion-resources/src/resources/http_route.rs`: `MirrorTuning::resolve_limits` defines
  the shared domain; `normalize_limits` supplies defensive per-field defaults.
- `http_route_preparse.rs`: HTTPRoute annotations use the same domain and
  preserve the established diagnostic/default fallback for invalid values.
- `edgion_plugins/plugin_configs/request_mirror.rs`: strict plugin validation
  reaches both Controller status and Gateway compilation through
  `EdgionPlugin::validation_error`. Invalid plugin resources keep the existing
  fail-closed compilation policy; this differs from runtime mirror overload.
- `edgion-gateway/src/plugins/http/request_mirror/budget.rs`: a process singleton survives
  instance replacement; task admission never queues. Each owned byte buffer
  stores its permits in the `Bytes::from_owner` owner, so the final transport
  clone releases the allocation before returning the budget.
- `pg_request_body_filter.rs`: reserve a channel slot before copying; byte
  admission never waits. Failure cancels the mirror before closing its stream,
  preserving the original request bytes and the callback's unconditional Ok.
- `ctx.rs` / `pg_request_body_filter.rs`: every terminal tee outcome retires
  mirror ownership instead of retaining an `Abandoned` sentinel. Cancellation
  outcomes signal their reason before closing the channel; a closed writer is
  disarmed before its handles are dropped; normal completion closes the sender
  before disarming and detaching the writer. The mirror source bit is cleared,
  and the optional body-plugin state is released only when no handler, WAF, or
  observer source remains. Handler/WAF replay-denial latches and co-resident
  observer state are preserved.
- `request_mirror/plugin.rs`: a snapshot reader is admitted and isolated
  synchronously before spawning. Its memory permit outlives the reader, and
  each emitted send chunk has an independent charge.
- `request_body/storage.rs`: isolation copies visible memory ranges into
  exact owned payload and retains disk ranges as disk ranges. Counting only
  snapshot logical length or cloning a small `Bytes` slice is insufficient:
  either can retain a much larger backing allocation.

Canonical authority: `skills/00-design-rules.md` sections D, U, V, W and
`AGENTS.md` HTTP-body invariants. This implementation reuses canonical
BodyStorage snapshots, its reader and storage IO lane; it creates no second
mutable request cache and never aggregates mirror responses.

## Alternatives and limits

The 64MiB budget is retained mirror body payload, not total RSS. Allocator and
queue metadata, connection/TLS buffers and canonical storage IO scratch are
separate. Each isolated snapshot disk read retains a canonical storage IO
permit with its raw output, including completed results waiting in a
JoinHandle and returned Bytes clones. This bounds outstanding mirror disk
scratch to 32 reads of at most 64KiB, even if a blocking read finishes after
cancellation. Other snapshot consumers keep their existing output ownership.
Do not claim that this temporary scratch is charged to the mirror payload
budget. Counting only running syscalls would be insufficient because completed
but unconsumed output can also accumulate.

Copying admitted tee chunks and snapshot memory ranges costs CPU and bounded
memory, but establishes ownership that shared `Bytes` slices cannot prove.
Non-mirror consumers keep the original zero-copy snapshot behavior. Disk-only
snapshots can stream uploads larger than either retained-byte budget; a
snapshot with memory segments additionally needs room for those segments and
the current send chunks. Insufficient capacity skips or cancels the mirror.

The standard Gateway API filter and vendored CRD are unchanged. New route
tuning travels through `edgion.io/mirror-max-buffered-bytes`; EdgionPlugins
uses inline `maxBufferedBytes`. Runtime overload continues the main request;
it does not turn a partial mirror upload into successful clean EOF. Byte
exhaustion has the bounded `buffer_limit` outcome on the existing counter and
mirror access log. Admission skips remain in the primary PluginLog.

## Verification and re-evaluation

Fresh verification on 2026-09-08:

- `cargo test -p edgion-gateway --lib mirror`: 32 passed, covering configuration
  boundaries, Controller/Gateway parity, defensive construction, shared
  cross-generation admission, final-clone ownership, disk streaming, and
  cancellation without modifying the main request.
- `cargo test -p edgion-gateway --lib request_body::`: 121 passed, including
  memory isolation and storage IO output lifetime, timeout, and rollback tests.
- `cargo test -p edgion-resources --lib mirror_`: 9 passed.
- `./cicd/checks/check_crd_coverage.sh`: 13 resource coverage and 8 plugin
  condition coverage tests passed.
- Workspace format, check, and Clippy passed; Clippy retains existing warnings.
  Agent-doc consistency, SSA force, metrics inventory, Pingora source policy,
  Gateway no-tracing, and diff whitespace guards passed.
- Independent read-only review: LGTM after correcting storage IO output permit
  retention and the documentation's latency claim.

These are targeted tests, not a full-suite or end-to-end integration run.
Existing integration scenarios and the vendored Gateway API CRDs are unchanged.
Historical campaign and BODY-014 test results are separate evidence.

Fresh verification for terminal retirement on 2026-09-09:

- `cargo test --locked -q -p edgion-gateway --lib request_body_plugin_state_tests -- --test-threads=1`:
  6 passed.
- `cargo test --locked -q -p edgion-gateway --lib routes::http::proxy_http::pg_request_body_filter -- --test-threads=1`:
  14 passed.
- `cargo test --locked -q -p edgion-gateway --lib routes::http::proxy_http::pg_request_body_filter_action -- --test-threads=1`:
  5 passed.
- `cargo test --locked -q -p edgion-gateway --lib mirror -- --test-threads=1`:
  38 passed.
- `cargo test --locked -q -p edgion-gateway --lib routes::http::proxy_http -- --test-threads=1`:
  244 passed.
- `cargo check --locked -q -p edgion-gateway --all-targets`, workspace format
  check, diff whitespace check, agent-doc validation, and the Gateway
  no-tracing guard passed.

These targeted tests pin terminal mirror absence for complete,
downstream-abandoned, channel-full, byte-limit, closed-writer, and defensive
pre-abandoned inputs. They verify that later chunks preserve the main request
bytes and recorded abandon reason, mirror-only retirement restores the outer
null-state fast path, and co-resident handler/WAF/observer sources remain active.
This is unit evidence only; no end-to-end suite was rerun for the terminal-state
retirement change.

Re-evaluate if snapshot ownership, Bytes transport conversion, semaphore
capacity, mirror configuration lifecycle, or response handling changes. A new
full-body cache, early permit release, or per-generation global budget violates
the scope of this correction.

## Resolution references

- D0066 — confirmed configuration-correctness defect, fixed after original deferral.
- `bound-request-mirror-concurrency-and-buffering` — canonical task identifier.
- BODY-014 — [committed body semantics](request-mirror-committed-body-snapshot.md).
