# Evaluate local-file identity probe cost before changing cadence

Status: deferred for user decision. Priority: P3. Type: performance investigation.
Inspected baseline: `dd1d22ac3`. No runtime or recovery-policy change.

## Evidence

The LocalFileWriter worker in
`edgion-gateway/src/link_sys/providers/local_file/data_sender_impl.rs` calls
`reopen_sink_if_renamed` before each batch. Its unchanged-path fast path still
calls `active_path_matches`, which reads active-path filesystem metadata. A batch
contains the first received line and up to 999 immediately available lines; it
does not wait to fill 1000 entries. Sparse traffic can therefore pay one probe
per line. Idle timeouts also probe attachment and drive disk/flush recovery.

This work runs on the dedicated writer thread, not the request executor.
`DataSender::send` moves the owned String into a bounded nonblocking queue; the
worker borrows each line for buffered writing. There is no extra String clone at
this handoff. Probe cost may affect queue drainage, but neither a bottleneck nor
a throughput improvement has been measured.

The probe is a correctness mechanism: Unix compares descriptor-derived device
and inode with the active path, including rename followed by recreation. On a
mismatch the worker flushes retained bytes to the old writer, opens and validates
the candidate, then publishes its identity and length. Metadata/open failures
leave attachment unhealthy for retry. Non-Unix recovery remains presence-only.
Concurrent external changes are not atomic with validation.

## Decision and acceptance

First measure filesystem metadata latency, actual batch-size distribution,
writer CPU, queue saturation and recovery behavior under sparse and saturated
traffic. Include supported deployment filesystems and rename/backfill activity.
Retaining the current checks is the default outcome unless measured benefit
justifies a separately reviewed design.

Moving the probe to the rotation/disk-check cadence can leave more records on an
old renamed or already-backfilled file before recovery. This is not an equivalent
optimization and requires an explicit user-approved recovery contract. An event
notification design would need its own portability, missed-event, shutdown and
fallback analysis; this task does not authorize a watcher or new configuration.
Do not replace identity checks with existence checks or assume an application
notification covers external filesystem changes.

Preserve before-batch and idle attachment checks unless that precise contract is
explicitly changed. Retain flush-before-replacement, candidate-path validation,
failure health transitions, retry without incoming traffic, and internal
rotation's confirmed-attachment gate. Keep size rotation per batch, disk probes,
drop accounting and first-healthy routing independent of the proposed probe
strategy. Queue saturation must not introduce Tier-2 send-fallback. Do not claim
exactly-once backfill or atomic protection from concurrent renames.

Original authority:
`skills/04-review/link-sys/intentional-design/access-log-fallback-backfill-tradeoffs.md`
(ARCH-026 recovery evidence). Any implementation needs a separate IMPL plan,
exact source allow-list and independent review. Acceptance must cover rename,
rename-plus-recreate, retained-buffer flush failure, candidate/open/metadata
failure, idle recovery and internal rotation, with explicit bounds on any newly
permitted delay or affected records. Performance measurement must include those
correctness costs rather than reporting only fewer metadata calls.

Fresh unchanged-code verification passed 24 local-file sender tests, formatting,
workspace all-target check and Clippy. Existing unrelated warnings remain.
These tests protect current behavior; they do not validate a changed cadence.
No full suite, live integration or performance benchmark ran.
