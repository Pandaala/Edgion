# D0018 — conf_sync client expiry identity mismatch

- Status: needs-investigation
- Canonical task: `tasks/todo/conf-sync-client-expiry-watch-installed-identity.md`
- Discovered in: B0080 analyst `b0080_analysis`; independent batch review pending.
- File expiry can advance while rejected/stalled reload retains the old Channel. Cross-crate ownership design deferred; safe campaign continuation confirmed.
