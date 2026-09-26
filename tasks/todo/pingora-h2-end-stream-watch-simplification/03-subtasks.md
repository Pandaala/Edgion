# Subtasks

| ID | Scope | File ownership | Status | Output |
|---|---|---|---|---|
| A1 | Current/minimum h2 watched/unwatched baseline and mutations | `pingora-core/src/protocols/http/v2/client.rs` tests only | in-progress | 0.4.16/0.4.19 terminal and informational matrix recorded; writer-first minimum mutation remains |
| A2 | Core raw-wire state matrix for terminal HEADERS, informational responses, RST, GOAWAY, and lifecycle | `pingora-core/src/protocols/http/v2/end_stream_watch.rs` tests only | in-progress | Current scanner defects characterized; final poison contracts await implementation |
| A3 | Proxy cache-admission and same-upstream-connection reuse harness | Read-only analysis, then proxy integration tests after design approval | completed | Concrete `MemCache` state probe, cache-proxy observability, window control, and multiplex raw/API origin plan |
| D1 | Narrow-adapter design and dependency policy | Edgion task documents | in-progress | Production gate closed: both supported h2 versions and upstream master require decoder-level trailer pseudo-header rejection before simplification |
| D2 | h2 decoder remedy decision | upstream h2 | deferred | No local fork; wait for an upstream release that passes the trailer contracts |
| I1 | Narrow wire observer implementation | `pingora-core/src/protocols/http/v2/end_stream_watch.rs` | pending | Remove DATA accounting/cache/padding state; retain terminal HEADERS, writer notification, irreversible local invalidation, persistent GOAWAY ceiling/poison, and EOF/error terminalization |
| I2 | H2 client integration | `pingora-core/src/protocols/http/v2/client.rs` production and tests | pending | Public h2 read completion, terminal-HEADERS state machine, permanent error latch |
| I3 | Connector and proxy writer integration | `pingora-core/src/connectors/http/v2.rs`, `pingora-proxy/src/proxy_h2.rs` | pending | Writer notification breaks blocked capacity waits; shutdown connections cannot be selected or allocate new streams |
| I4 | Cache and upstream-reuse integration | `pingora-proxy/tests/` and test utilities | pending | Failed fills never hit; reusable stream-local cases accept a second stream on the same H2 connection |
| I5 | Dependency gate and fork docs | `AGENTS.md`, `docs/edgion-fork/` | deferred | Fork policy recorded; rerun contracts when upstream h2 changes |
| R1 | Independent production code review | Read-only final diff | pending | Must Fix resolution and accepted residual risks |
| V1 | Targeted tests, format, check, clippy, docs | Whole Pingora task diff | pending | Recorded commands and results |

## Ownership constraints

- The staged poll trailer-error latch in `client.rs` predates this task and must remain intact.
- The staged PROXY protocol change and untracked `.superpowers/` directory are out of scope.
- Production simplification remains deferred until upstream h2 resolves D2 and
  the revised design passes an independent design review.
- No two implementation agents may edit the same file set concurrently.
- I1 must retain shared lock ownership for registration, publication, GOAWAY ceiling, and poison. I2 must include direct-trailer permanent-latch coverage. I3 must prove EOS-without-reset capacity wakeup and shutdown-pool exclusion. I4 must include Content-Length-underflow/drop and exact-length-invalid-trailer cache negatives, a locally cancelled-before-late-EOS race, and connection-level GOAWAY non-reuse.
