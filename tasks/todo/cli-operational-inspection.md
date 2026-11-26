# CLI operational inspection

Deferred product work from common-05, following delivery of the two Controller diagnostics commands.

- Route status: present route conditions with their parent and controller identity. Cover different outcomes for two parents and absent status.
- Fleet sync status: establish how to obtain per-Gateway, per-kind applied versions before designing a command. The current Watch stream has no ACK. Bind observations to the Controller instance and distinguish stale or unavailable observations from convergence. See [the accepted protocol boundary](../../skills/04-review/conf-sync/intentional-design/no-ack-nack-control-plane-validation.md).
- Configuration diff: compare manifests with persisted Controller resources without writing. Define treatment of generated fields, absent objects and sensitive values. Server-side dry-run validates a document but does not compute a diff.
- Target names: evaluate clearer names for Controller CRUD, Controller cache and Gateway cache, accounting for scripts, docs and tests before changing the public interface.
