---
name: h2-shutdown-connection-not-allocatable
description: Historical review evidence for Pingora fork / H2 connection pool; revalidate against current code before applying.
knowledge_type: regression-guards
---

# H2 shutdown connection not allocatable

## Recorded conclusion

`mark_shutdown()` is enforced where a stream is allocated (`ConnectionRef::spawn_stream`, checked before `new_stream()`, plus a defensive post-await re-check that cannot fire while `Stub::new_stream` clones `SendRequest`) and where the pool picks a candidate (`Connector::reused_http_session`), not only at re-pooling. "`more_streams_allowed()` already checks the flag" is not accepted: it runs after the stream is handed out. Nothing evicts a marked connection from the in-use pool until its last stream is released, so a read-timeout mark used to amplify one hung upstream stream into a run of them. The guarantee is at the allocation boundary, not the whole request: the `spawn_stream` -> `send_request` gap remains open by design, matching GOAWAY's "committed work finishes" semantics. Filtering at selection only, guarding inside `spawn_stream` only, and returning an error instead of `Ok(None)` are also not accepted.

## Evidence boundary

This decision was migrated from an earlier review record. The historical source paths,
verification claims, and assumptions describe that resolution, not a new code audit.
Before applying it to a new finding, trace the current implementation and tests.
Re-evaluate if the cited mechanism, dependency, or threat model changes.

External reference: [h2-grpc/h2-shutdown-connection-not-allocatable.md](https://github.com/Pandaala/pingora/blob/edgion_v3/edgion-changes/review/h2-grpc/h2-shutdown-connection-not-allocatable.md)

## Resolution references

- `H2-008` — 2026-08-27 (fixed in pingora fork `edgion_v3`).
