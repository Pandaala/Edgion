---
name: conf-sync-cert-watch-mtime-length-signature
description: Use when reviewing conf_sync file-mode certificate watcher findings that propose hashing PEM content instead of the (mtime, length) reload signature.
status: accepted-tradeoff
finding_id: conf-sync-cert-watch-content-signature
closed: 2026-09-04
knowledge_type: intentional-design
---

# conf_sync File-Mode Cert Watch Keeps `(mtime, length)` — ACCEPTED-TRADEOFF

## Conclusion

File-mode conf_sync certificate hot reload on both the Controller and the Gateway
continues to gate reload on a per-file `(mtime, length)` signature. That signature is
shared by the notify-event path and the 5-minute re-stat backstop. A same-size
certificate, key, or CA replacement that also preserves modification time can be
skipped; a metadata-only touch can still trigger one extra reload. Both outcomes are
accepted.

Fix suggestions of the form "replace length with a content hash", "hash cert/key/CA
bytes on every directory event", or "read and digest the three files on every restat
even when metadata is unchanged" are **not accepted**.

## Core Rationale

**1. The watcher already sees filesystem events; the signature is a reconnect filter.**

Both watchers observe the parent directories of the active cert/key/CA paths, debounce
Secret-mount `..data` bursts for 2 seconds, and only then decide whether to reload.
A successful Gateway reload rebuilds the gRPC channel and cancels every conf_sync
watch stream. The `(mtime, length)` comparison exists so a no-op directory event does
not become a reconnect storm. It is not a claim that metadata equals content.

Sources: `edgion-gateway/src/conf_sync/conf_client/cert_watch.rs`,
`edgion-controller/src/conf_sync/conf_server/cert_watch.rs`.

**2. The residual miss is a rare conjunction, not the common rotation path.**

Ordinary writes, atomic renames, and Kubernetes Secret `..data` swaps give the
replacement a new mtime. Same-length PEM rotation is common; same-length *and*
unchanged mtime together requires preserved timestamps, coarse filesystem resolution
in the same second, or unusual copy-in-place behavior. Controller `auto_ca` does
not call this file watcher (it uses identity-sync). Gateway `auto_ca` still does
when cert paths are mounted; the usual kubelet `..data` swap updates mtime, so
the same residual miss remains rare.

**3. A metadata-only extra reload is operationally acceptable.**

If content is unchanged and only mtime moved, validate-before-swap still installs the
same identity. Gateway reconnect churn is brief and control-plane only. That cost does
not justify hashing PEM material on every directory event or every 5-minute backstop.

## Fix Suggestions Not Accepted

- Content digest as the reload signature (event path, restat path, or both) — the
  miss it closes is almost never observed, and it adds a shared Controller/Gateway
  decision model plus PEM reads on directory noise.
- Reloading on every directory event without a signature — Secret-mount bursts and
  unrelated directory metadata would recycle conf_sync streams.
- Shortening `RESTAT_INTERVAL` to compensate — the backstop already uses the same
  metadata tuple; polling faster does not see content the signature ignores.

## Re-evaluation Triggers

Re-open this decision only if:

- Operators report a file-mode rotation that landed on disk but was ignored until
  process restart, with evidence that mtime and length did not move.
- A supported cert-distribution mechanism is documented to replace files in place
  while preserving both size and modification time.
- Gateway cert reload stops cancelling watch streams, so a content-hash comparison
  would no longer carry reconnect cost.

## Reference Cases

- Closed `tasks/todo/conf-sync-cert-watch-content-signature.md` on 2026-09-04
  without a runtime change.
- Campaign note: `tasks/pending/edgion-gateway-file-optimization/07-changelog.md`
  (CONF-SYNC-CLIENT-001 deferred the content-signature recovery; this file is the
  later wont-fix record).
- Operator-facing hot-reload contract:
  `skills/02-features/02-config/01-gateway-config.md` (mtime/length re-stat backstop).

## Resolution references

- `conf-sync-cert-watch-content-signature` — 2026-09-04 (accepted tradeoff).
