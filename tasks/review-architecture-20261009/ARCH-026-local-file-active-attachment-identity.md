---
id: ARCH-026
status: candidate
kind: behavior-defect
priority: P2
primary_partition: PART-276
independent_verification: confirmed
---

# Compare LocalFile writer attachment with the current active file

## Current ownership and missed transition

local_file/data_sender_impl.rs:290 implements reopen_sink_if_renamed_with.
It returns immediately when sink.path_ok && base_path.exists(). path_ok records
known failures; it does not establish that the open writer targets the file now
found at the active path.

Start a healthy writer, externally rename the active file, then recreate the
active path before the next batch or idle callback. path_ok remains true and
base_path exists, so every callback skips replacement. Until another operation replaces the writer, later bytes continue landing in
the renamed inode while the provider reports healthy; with the supported Never
rotation strategy this can persist indefinitely.
This is the ordinary rename-plus-create sequence claimed by the source and
accepted backfill knowledge. Rename-only recovery is implemented and is not a
new finding. No runtime reproduction or incident is claimed.

The existing unhealthy_attachment_reopens_externally_recreated_base_and_recovers
test manually sets path_ok=false; it exercises recovery after a known failure
and does not cover this healthy-to-detached transition.

## Bounded improvement and alternatives

Keep opened-file identity in the private writer and compare it with the current
canonical path during existing background recovery checks. Descriptor/path
device-and-inode identity is a bounded approach on supported Unix targets.
Reuse flush-before-replace, failed-reopen health and rollback. Generic failing
writer tests can receive captured identity separately; do not burden production
with an indirection for mocks.

Existence alone cannot prove attachment. Waiting for a write error also fails:
an open renamed file remains writable. A new watcher framework is unnecessary
because recovery already runs without request traffic. Account for metadata/open
races and platform support explicitly; do not promise perfect external rotation
atomicity from one comparison.

## Invariants, knowledge correction and future verification

Preserve retained-byte flushing, atomic no-overwrite archives, recovery while idle,
per-batch size rotation, rollback and typed drop counters. Accepted queue saturation,
disk thresholds, buffer-recovery size drift and process-exit delivery remain intact.
No request tracing or registry is added.

The original fallback/backfill page claims the existing behavior also covers
logrotate create. Current existence checking supports a narrower claim; correct
that owning knowledge alongside a future implementation. Healthy semantics,
write-error and per-batch rotation records do not accept attachment to the wrong
active file. Future tests must rename and recreate while path_ok stays true,
prove subsequent lines reach the new active file, and retain failure/recovery
coverage. Static source/test inspection only; source and knowledge receipts live
in the partition report and independent verification.
