---
name: fs-conditional-delete-validate-before-unlink
description: Use when reviewing FileSystemStorage conditional-delete ordering findings; covers why delete_one_if_version validates the precondition before unlinking rather than isolating the file first.
status: fixed
finding_id: storage-01
closed: 2026-07-17
---

# FileSystem conditional delete validates before it destroys — FIXED

## Conclusion

`FileSystemStorage::delete_one_if_version`
(`edgion-controller/src/conf_mgr/conf_center/file_system/storage.rs`) reads the
current file, checks `ensure_expected_resource_version`, and only then calls
`fs::remove_file`. It does **not** rename the live file to a tombstone first.

Fix suggestions of the form "rename the file to a tombstone first, then validate
the isolated inode, then restore it if the precondition fails" are **not
accepted** — that was the previous implementation and it is what storage-01
removed.

## Core Rationale

**1. The rename-first ordering bought nothing and cost a data-loss window**

The old code moved the live file to `<name>.delete.N.tmp`, read the tombstone,
validated, and restored via `restore_isolated_file_no_clobber` when the caller's
`If-Match` did not match. So a delete destined for a 409 still unlinked the live
resource for the duration of a read plus a rename. If the process died inside
that window (crash, SIGKILL, eviction, OOM) the resource was stranded as a
`.tmp` file — and `.tmp` is skipped by both the init scan and `list`, so it was
unrecoverable. A rejected no-op operation could permanently delete an operator
resource.

**2. The TOCTOU it guarded against is outside FileSystem mode's model**

The original comment justified the ordering as closing a TOCTOU: a read/compare
followed by `unlink(path)` lets an external writer replace `path` between the two
syscalls, making the delete hit the replacement. That reasoning is sound in the
abstract but the premise does not hold here:

- FileSystem mode runs `run_lifecycle_gated(..., leadership: None)`
  (`non_kube_common.rs:173`) — the unconditional single leader. There is no
  second controller replica writing this tree.
- The per-key `update_lock` (`storage.rs`, `UPDATE_LOCKS`) is taken by `set_one`,
  `create_one`, `update_one_if_version` and `delete_one_if_version` alike, so
  every in-process writer to the key is already serialized against this delete.

The only actor left is a human with a text editor or an out-of-band process.
For that actor the rename-first ordering traded a **certain** loss window for an
**uncontrolled** one — deleting a file someone replaced 200µs ago is not
meaningfully better, and both are outside the model.

**3. It now matches its sibling under the same lock**

`update_one_if_version` two functions up already does read →
`ensure_expected_resource_version` → write under the same `update_lock`. The
delete path is now structurally identical, and the conditional and unconditional
branches collapse into one `remove_file`.

## Fix Suggestions Not Accepted

- "Restore the isolate-then-validate ordering to close the unlink TOCTOU" — reintroduces
  the crash-window data loss described above; the writer it guards against cannot exist
  under single-leader + `update_lock`.
- "Keep the tombstone but fsync/journal it so a crash is recoverable" — solves a
  self-inflicted problem; no isolation step means nothing to recover.
- "Apply the same reorder to the etcd backend" — **wrong**. `txn_delete_if_mod_revision`
  (`etcd/storage.rs`) is a real atomic CAS and is correct as written.

## Re-evaluation Triggers

Re-open this decision only if:

- FileSystem mode gains multi-writer semantics (a real leader election with more than one
  replica writing the same tree, or a supported out-of-band writer contract).
- The per-key `update_lock` is removed or stops covering one of the four write paths.

## Reference Cases

- center-resource-ui-contract review (2026-07-16), finding P2 → storage-01, fixed 2026-07-17.
- `edgion-controller/src/conf_mgr/conf_center/file_system/storage.rs` — `delete_one_if_version`,
  `update_one_if_version`, `UPDATE_LOCKS`.
- `edgion-controller/src/conf_mgr/conf_center/non_kube_common.rs:173` — `leadership: None`.
