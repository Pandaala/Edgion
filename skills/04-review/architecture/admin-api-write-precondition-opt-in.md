---
name: admin-api-write-precondition-opt-in
description: Use when reviewing Controller Admin API optimistic-concurrency findings; covers why the CAS precondition on PUT/DELETE stays opt-in (last-writer-wins documented, not enforced) instead of returning 428 Precondition Required.
status: accepted-tradeoff
finding_id: new_work-16
closed: 2026-07-28
---

# The Controller Admin API's Write Precondition Is Opt-In — ACCEPTED-TRADEOFF (documented, not enforced)

## Conclusion

A `PUT`/`DELETE` on `/api/v1/{namespaced,cluster}/...` that carries neither an `If-Match` header
nor a body `metadata.resourceVersion` unconditionally replaces every caller-owned field, and that
stays the contract. The chosen resolution is to state the last-writer-wins guarantee explicitly to
callers (Option C of the finding), not to require the precondition.

Fix suggestions of the form "return `428 Precondition Required` when no precondition is supplied",
"require it for `EdgionConfigData` because that is the one remotely-writable kind", or "require it
for every kind since we are pre-release anyway" are **not accepted**.

## Core Rationale

**1. The current behavior is deliberate Kubernetes parity, not an oversight.**

The Kubernetes API's update verb is likewise unconditional when `metadata.resourceVersion` is
empty; the precondition is opt-in there in exactly the same way. (That is upstream Kubernetes
behavior — it is external knowledge, not something this repository asserts or tests, so treat it as
the premise it is rather than a citable local fact.) The whole `ConfCenter` abstraction
(`edgion-controller/src/conf_mgr/conf_center/`) exists so that the Kubernetes, etcd, and
FileSystem backends are interchangeable behind one API, and the Admin API mirrors Kubernetes
semantics on purpose. Requiring the precondition would make this API stricter than the API it
mirrors.

Do not reach for "then scope the `428` to the backend that needs it" (FileSystem is the only one
with no CAS of its own). It is technically available — the handler can branch on
`state.is_k8s_mode()`, and `state.conf_dir_registry()` returns `Some` only in FileSystem mode, which
`diagnostics_handlers::conf_conflicts` already branches on exactly that way — but a per-backend HTTP
contract is itself the thing to avoid: the same request would succeed or `428` depending on the
deployment's storage choice. That is precisely the contract fork the `ConfCenter` abstraction exists
to prevent.

**2. In K8s mode the guarantee would be unenforceable at the real boundary.**

There, the storage *is* the API Server, and the Admin API is one of several write paths to the
same CRD — an operator running `kubectl apply` never passes through
`expected_resource_version()` at all. A `428` on the Admin API would advertise a lost-update
guarantee that a second, equally normal write path walks straight around. The machinery that does
work is unchanged and stays available: `If-Match` → `ConfWriterError::Conflict` → `409`, enforced by
all three backends inside their atomic windows — file-system compares inside its per-key
`update_lock`, etcd compares and then fuses the write into a `txn_put_if_mod_revision`, and
Kubernetes compares the fetched object (`ensure_expected_resource_version_value`) before a replace
that carries the fetched native `resourceVersion`; `Preconditions` proper appears on the
`delete_one_if_version` path, not on the replace.

Note the corollary, which the Admin API skill now spells out: because etcd's mod-revision
transaction and the Kubernetes replace both guard the backend's own get→write window regardless of
what the client sent, a precondition-free caller can still receive a `409` it did not ask for. That
window is orders of magnitude narrower than the caller's cross-request read→modify→write, so it does
not amount to lost-update protection — but "no `409`" must not be read as "no concurrent writer".

**3. The kind the finding worried about has a single intended writer, and that writer already sends it.**

`EdgionConfigData` is the Center-controlled data overlay with one intended writer — settled the
same day by finding 15, see
[`edgion-config-data-managed-by-is-inert.md`](edgion-config-data-managed-by-is-inert.md). That writer
supplies the precondition: `if-match` is in the federation proxy header allowlist
(`edgion-controller/src/fed_sync/fed_client/mod.rs`, `ALLOWED_PROXY_HEADERS`), and Center's dedicated
`EdgionConfigData` write path refuses to dispatch a write with no version
(`center-app/src/api/config_data_ops.rs`, test `missing_document_or_version_fails_before_dispatch`).
Be precise about the scope of "always", though: Center's *dashboard* generic resource `PUT` sends
`If-Match` only when the edited document carries a `resourceVersion`
(`web/src/api/resources.ts`, `resourceVersionHeader()` returns `{}` otherwise), whereas its generic
delete is unconditionally guarded (`requiredResourceVersionHeader()` throws). The sibling entry's
"the dashboard's generic CRUD sends `If-Match` too" is best-effort in the same way. Either way the
scenario a `428` would protect — two writers contending for one overlay document — is not in the
design, while the cost of the `428` lands entirely on legitimate ad-hoc operations.

**4. The real gap was a documentation-honesty gap, and it is closed.**

`skills/01-architecture/01-controller/02-admin-api.md` previously framed the absent precondition as
a backwards-compatibility concession ("preserves unconditional behavior for older clients") and
never said what a caller does *not* get. It now states plainly that such a write unconditionally
replaces every caller-owned field (`merge_operator_update_*` takes `apiVersion`, `kind`, and
`status` from the stored object and preserves server-created metadata — bar the request's
`resourceVersion`/`generation` on the non-K8s backends — so it is not literally a whole-object
replace), that another
client's update is silently discarded while this caller still gets `200`/`204`, that neither
`If-Match: *` nor `api/resource_locks.rs` is protection — the latter makes a loss deterministic
rather than detected and is taken on only two paths — and that detection of the caller's own
conflict is the caller's responsibility. It also names the callers that send nothing today:
`edgion-cli apply` (`commands/apply.rs`, which performs a `GET` and uses only its status code),
`edgion-cli delete`, and the `edgion-tests` `apply_yaml` helper (`client/framework.rs`).

The doc comments at the two source locations the finding named — `expected_resource_version()`
(`api/common.rs`) and `ensure_expected_resource_version_value()`
(`conf_mgr/conf_center/common.rs`) — plus the `ConfWriter` trait contract (`conf_center/traits.rs`)
carried the same "for legacy clients" framing and were corrected to match. Left out of this
change and handled by task 19 (closed the same day): `api/resource_locks.rs`'s module doc, whose
claim that "neither the FileSystem nor the Kubernetes backend rejects a stale write via
`resourceVersion` on this path" was wrong for all three backends. It now says what the lock
actually buys on top of the precondition — see
[`../concurrency/failover-read-modify-write-per-key-lock.md`](../concurrency/failover-read-modify-write-per-key-lock.md).

## Fix Suggestions Not Accepted

- **Return `428 Precondition Required` for `EdgionConfigData` only (finding's Option A)** — forks
  the Admin API contract per kind, breaks `edgion-cli apply` and the `edgion-tests` `apply_yaml`
  helper for that kind, and protects a contention scenario the kind's single-writer design does not
  admit. Unenforceable in K8s mode (rationale 2).
- **Return `428` for every kind (finding's Option B)** — same objections plus a much larger blast
  radius: every scripted caller, `edgion-cli apply`/`delete`, and the integration-test helpers break
  at once.
- **Reject `If-Match: *` instead of treating it as unconditional** — moot once the precondition
  stays opt-in, and the current reading is already truthful: `*` means "must exist", and existence
  is enforced by the update/delete path itself (`ConfWriterError::NotFound` → `404`).
- **Make `edgion-cli apply` send `If-Match` from the `GET` it already performs** — not accepted as
  part of this decision. `apply` is declarative "make it so" (matching `kubectl apply`, which is
  also precondition-free); turning it into a CAS operation would surface `409` on an ordinary
  re-apply. Should a CLI conflict-detection mode ever be wanted, it belongs in a separate task as
  an explicit flag, not as a silent change to `apply`.
- **Drop the body `metadata.resourceVersion` fallback so `If-Match` is the only form** — the
  fallback is the compatibility path for clients that round-trip a fetched object, and removing it
  narrows nothing (an absent precondition remains legal either way).

## Re-evaluation Triggers

Re-open this decision only if:

- A second writer of `EdgionConfigData` is introduced (a GitOps reconciler, a second Center, or an
  operator-facing authoring flow expected to survive a Center write) — this is the same trigger
  that governs [`edgion-config-data-managed-by-is-inert.md`](edgion-config-data-managed-by-is-inert.md).
- A real lost-update incident is reported against the Admin API, i.e. two non-Center writers
  observably clobbering each other in production.
- The Admin API stops mirroring Kubernetes write semantics, or the Kubernetes backend is retired so
  that the Admin API becomes the only write path to configuration.
- `edgion-cli` gains an explicit conflict-detecting write mode; the CLI half of this entry then
  needs rewriting.

## Reference Cases

- `EdgionCenter/tasks/new_work/00-index.md` §16 (closed 2026-07-28; the finding offered three
  options and Option C — document the contract — was chosen, with the two enforcement options A and
  B declined for the reasons above). The per-issue file
  `16-controller-write-preconditions-are-opt-in.md` was deleted on close, per that index's "a task
  file that no longer exists in this directory is done" convention; the index section is the
  surviving record.
- `edgion-controller/src/api/common.rs` — `expected_resource_version()`, the resolution order
  (`If-Match` → body `metadata.resourceVersion` → none).
- `edgion-controller/src/conf_mgr/conf_center/common.rs` — `ensure_expected_resource_version_value()`,
  the `None` → `Ok(())` early return that makes the write unconditional.
- [`../../01-architecture/01-controller/02-admin-api.md`](../../01-architecture/01-controller/02-admin-api.md)
  — the explicit last-writer-wins contract this decision produced.
- [`edgion-config-data-managed-by-is-inert.md`](edgion-config-data-managed-by-is-inert.md) — the
  single-intended-writer decision for `EdgionConfigData` (finding 15).
- [`../concurrency/fs-conditional-delete-validate-before-unlink.md`](../concurrency/fs-conditional-delete-validate-before-unlink.md)
  — how the file-system backend enforces a *supplied* precondition.
