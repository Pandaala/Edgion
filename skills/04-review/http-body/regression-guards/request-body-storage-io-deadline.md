---
name: request-body-storage-io-deadline
description: Use when reviewing request-body spill, replay, materialization, snapshot reads, their blocking-pool admission, or storage timeout cleanup.
knowledge_type: regression-guards
---

# Request-Body Storage I/O Has a Separate Bounded Lane

## Conclusion

Every ordinary disk-backed `BodyStorage` file operation is admitted through a
fixed process-wide semaphore before it is submitted to Tokio's blocking pool.
Admission and execution share the request's resolved non-zero
`storageOperationTimeout` deadline. Explicit plugin scan jobs retain their
separate scan semaphore because their CPU/scanner lifetime is not a single
storage operation.

## Cancellation and Ownership

Timing out an awaited `spawn_blocking` task cannot interrupt a file-system
syscall that has already started. The blocking closure therefore owns its
storage permit until it really ends. A queued task is aborted when its waiter
is cancelled; an active task may finish late, but at most 32 storage operations
can remain queued or active. Once all permits are pinned, new requests expire
while waiting for admission and do not submit more blocking work.

Capture is transactional around blocking I/O: `BodyStorage` is changed to
`Poisoned` before spill creation or append, and the disk state plus captured
length are committed only after timely success. Spill creation uses an
anonymous/unlinked temporary file immediately; the blocking closure owns only
the file handle needed to finish safely, so an active syscall that outlives its
waiter cannot leave a named request-body file behind.

Replay uses positional reads and advances its cursor only through Pingora's
synchronous `consume` callback after a complete chunk is returned. Deadline,
short-read, join, or I/O failure returns an internal storage error, publishes no
chunk, and permanently fails replay for that request.

## Protocol Semantics

- Storage expiry is `BodyReadError::StorageFailure`, not client `Timeout`.
- Capture failure disables HTTP/1 reuse because unread downstream bytes may
  remain. Pingora's same keepalive operation is a no-op for HTTP/2; dropping the
  unfinished request resets only that stream, leaving sibling streams usable.
- A replay failure happens after complete capture and does not manufacture an
  incomplete EOF for the upstream.

## Re-evaluation Triggers

- The temporary storage backend gains an interruptible async file API.
- Storage concurrency becomes a process-start setting; do not make it a
  replaceable EGC hot-reload semaphore without preserving permits held by old
  snapshots.
- Explicit scan jobs receive their own wall-clock contract covering both
  scanner CPU and disk reads.

## Reference Cases

- `edgion-gateway/src/request_body/storage.rs`
- `edgion-gateway/src/request_body/storage_io_tests.rs`
- `edgion-gateway/src/plugins/runtime/session_adapter/storage_io_tests.rs`
- `skills/04-review/http-body/mandatory-design/request-body-policy-egc-hot-reload.md`
