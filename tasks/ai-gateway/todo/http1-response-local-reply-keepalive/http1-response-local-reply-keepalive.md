# Preserve HTTP/1 Keepalive After a Response-Stage Local Reply

## Meta

| Key | Value |
|-----|-------|
| Created | 2026-08-03 |
| Status | pending |
| Type | feature |
| Priority | P3 |
| Issue | `filter-04c #3`, originally reported by `gw-http-proxy-resp-03` |

## AI guide summary

Edgion currently closes an HTTP/1 downstream connection after a response-stage
plugin replaces an upstream response with a complete local reply. The behavior
is protocol-correct, but it prevents keepalive reuse and adds a connection setup
after every such reply. Explore and design a safe cross-repository extension to
the Edgion Pingora fork so that `FailToProxy.can_reuse_downstream` can authorize
reuse on the post-upstream-pump error path. The difficult part is preserving
accurate wire framing and keeping Pingora's codec-level safety checks as the
final authority while still suppressing all remaining upstream response bytes.

This is a performance feature, not a correctness bug. The current implementation
advertises connection close and then closes the socket, so clients do not pool a
connection that unexpectedly disappears.

## Requirements

- Decide whether and how the Edgion Pingora fork should propagate
  `FailToProxy.can_reuse_downstream` after an error returned from the upstream
  response pump.
- When a response-stage local reply has been completely written, allow a second
  HTTP/1 request to run on the same downstream connection if all lower-level
  Pingora reuse checks also approve it.
- Preserve the existing single-write behavior: the local reply is written once,
  and no upstream response header or body byte is forwarded afterward.
- Preserve accurate HTTP/1 framing. Never advertise keepalive when the connection
  will actually be closed.
- Treat the application reuse verdict as permission, not a command. Codec state,
  incomplete request bodies, shutdown, HTTP version, reuse limits, overread, and
  other transport safety checks must retain the ability to reject reuse.
- Keep HTTP/2 semantics unchanged; H2 stream lifecycle and multiplexed connection
  reuse are outside this feature.
- Add regression coverage in both the Pingora fork and Edgion.

## Classification

| Dimension | Assessment |
|-----------|------------|
| Product category | performance feature / enhancement |
| Current correctness | correct and conservative |
| Trigger | response-stage plugin produces a local reply after an upstream response header arrives |
| Frequency | expected low, workload-dependent |
| User impact | one additional HTTP/1 connection setup after each affected reply |
| Risk | medium because Pingora's common final-error lifecycle is involved |

## Current behavior

### Edgion response path

`edgion-gateway/src/routes/http/proxy_http/pg_response_filter.rs` owns the
single response-stage local-reply write point:

1. A response-stage plugin stashes a fresh local-reply header and body in the
   request context.
2. `response_filter` takes that reply, stamps the Gateway server identity, and
   marks the response committed.
3. It calls `session.set_keepalive(None)` so Pingora's HTTP/1 codec emits
   `Connection: close`.
4. It writes the complete reply.
5. It returns a sentinel `InternalError` to abort the upstream pump and prevent
   forwarding the replaced upstream response.

`edgion-gateway/src/routes/http/proxy_http/pg_fail_to_proxy.rs` recognizes the
sentinel through `ctx.local_reply_written`, avoids writing a second error page,
and returns:

```rust
FailToProxy {
    error_code: ctx.request_info.status.unwrap_or(500),
    can_reuse_downstream: true,
}
```

The `true` currently expresses intent only. Removing Edgion's explicit
`set_keepalive(None)` by itself is unsafe: the response would be framed as
keepalive-eligible while the post-pump path still drops the connection.

### Pingora post-pump path

The Edgion dependency and the local fork both currently point to:

- Repository: `Pandaala/pingora`, branch `edgion`
- Local checkout: `/Users/caohao/ws2/pingora`
- Commit: `b1d520ea24b4225ff3dcfa97b1493b919d363c13`

In `pingora-proxy/src/lib.rs`, `HttpProxy::process_request` receives
`server_reuse = false` when `response_filter` returns the sentinel error. It
later calls `ProxyHttp::fail_to_proxy`, but consumes only `error_code` for
logging. It passes the original `server_reuse` value to `HttpProxy::finish`, so
the downstream session is dropped instead of finished for reuse.

The pre-upstream `handle_error` path already honors
`FailToProxy.can_reuse_downstream` and calls `downstream_session.finish()`.
Searches found these two call sites as the only consumers of the field. The
post-pump call site is therefore the specific semantic gap.

`ProxyHttp::fail_to_proxy` currently documents that its reuse result is only
respected for failures before upstream peer selection. Expanding that contract
is part of this feature, not merely an Edgion-local bug fix.

## Existing lower-level safety gates

The design should reuse the existing Pingora lifecycle instead of duplicating
HTTP/1 checks in Edgion:

- `HttpSessionV1::write_response_header` disables keepalive when an early
  response is written before the downstream request body is complete and
  `close_on_response_before_downstream_finish` is enabled.
- `HttpSessionV1::reuse()` first checks `will_keepalive()`.
- If reuse remains allowed, `reuse()` drains unread request-body bytes.
- It refuses reuse when request bytes were overread, including unsupported
  pipelining cases.
- Graceful shutdown, HTTP/1.0 defaults, `Connection: close`, keepalive reuse
  limits, upgrades, and codec failures can independently disable reuse.
- `HttpSession::finish()` finishes the response body before attempting H1
  reuse; finishing an already complete local reply is expected to be
  idempotent.

The brainstorming phase must verify these assumptions against the exact fork
revision used for implementation and decide whether an additional
response-completeness guard belongs in the common Pingora path.

## Candidate direction for brainstorming

The smallest known direction is to retain the result returned by post-pump
`fail_to_proxy` and use its `can_reuse_downstream` value when selecting the
downstream finalization path. This would make the existing field effective in
both pre-pump and post-pump failures.

This is not yet a final design. Brainstorming should explicitly compare:

1. Extending the existing `FailToProxy` contract across both error paths.
2. Adding a more narrowly typed post-pump/local-response outcome.
3. Reusing Pingora's typed termination machinery instead of the current Edgion
   sentinel error.
4. Keeping the current conservative close behavior as an accepted performance
   tradeoff.

The preferred result should minimize public API growth, avoid a second response
write mechanism, and have a clear proof that a partially framed response cannot
be reused.

## Questions the design must answer

- Should `can_reuse_downstream` replace the pump's reuse verdict after a final
  error, combine with it, or be gated by an explicit response-complete fact?
- Is `response_body_finished()` sufficient for all supported downstream session
  types, or should the expanded contract remain application-trusted as the
  pre-pump path is today?
- Does calling `on_proxy_failure` before a successful H1 reuse have any
  observable side effect for H1, H2, Custom, or Subrequest sessions?
- What happens when the upstream replies before the downstream request body is
  complete? Verify that codec keepalive shutdown remains authoritative.
- Should the fork expose a typed control-flow result for a fully written
  response instead of using an error sentinel?
- Does cache cleanup differ when the pump exits through the sentinel error?
- Which Pingora tests prove both the positive reuse case and the negative
  codec-veto case?

## Scope

### In scope

- Response-stage local replies that are fully written downstream.
- HTTP/1 downstream lifecycle after the sentinel abort.
- Edgion Pingora fork changes required to express safe post-pump reuse.
- Updating the fork dependency revision in Edgion after the fork change has an
  explicit commit.
- Pingora self-contained regression tests.
- An Edgion integration test that reads two complete responses on one raw H1
  connection.
- Review knowledge describing the selected contract and its safety boundary.

### Out of scope

- Request-stage local replies, which use a different return path.
- HTTP/2 connection reuse.
- Sticky-session `Set-Cookie` carry-over. That is accepted as designed in
  `skills/04-review/architecture/local-reply-drops-sticky-set-cookie.md`.
- Pending response-header operation carry-over, which is already implemented.
- General filter-chain pause/resume or an asynchronous Pingora response-filter
  API.
- Retrying or forwarding any upstream response bytes after a local reply.

## Expected affected modules

| Repository / module | Expected impact |
|---------------------|-----------------|
| Pingora `pingora-proxy/src/lib.rs` | Propagate the final-error downstream reuse verdict |
| Pingora `pingora-proxy/src/proxy_trait.rs` | Define the expanded `FailToProxy` contract |
| Pingora `pingora-proxy/tests/test_request_body_seam.rs` | Same-connection positive and codec-veto regression coverage |
| Edgion `pg_response_filter.rs` | Remove the local-reply-specific keepalive shutdown only after Pingora supports reuse |
| Edgion `pg_fail_to_proxy.rs` | Update the obsolete limitation comment; preserve unrelated status handling |
| Edgion DSL integration suite | Exercise a real response-stage denial twice on one raw H1 socket |
| Edgion `Cargo.lock` | Pin all Edgion Pingora fork crates to the new common commit |
| `skills/04-review/` | Record the implemented or declined design and prevent stale re-reports |

## Test strategy

### Pingora fork

Add a self-contained test, preferably beside the existing request-body seam
tests, whose proxy implementation:

1. Receives an ordinary upstream response.
2. Writes a complete local response from `response_filter`.
3. Returns an error to stop the pump.
4. Returns `can_reuse_downstream: true` from `fail_to_proxy`.
5. Sends a second request over the same raw HTTP/1 socket and receives a second
   complete response.

Add a negative variant or equivalent assertion showing that
`session.set_keepalive(None)` or another codec condition still prevents reuse
even when the application returns `true`.

The raw response reader must consume exactly one complete HTTP/1 message before
sending the follow-up request. A marker-only read can mistake the tail of the
first response for the second response.

### Edgion

Extend the existing DSL response-stage denial coverage around
`dsl-resp-deny-upgrade.example.com`. Use `tokio::net::TcpStream` to:

1. Send a GET that reaches the upstream and is replaced by DSL
   `deny(403, "denied-by-response-plugin")`.
2. Parse the complete local reply and assert status/body/framing.
3. Send a second request on the same socket.
4. Parse and assert the second complete response.
5. Assert no upstream response bytes leaked after either local reply.

Do not use a pooled high-level client as the only proof; it may transparently
open a replacement connection.

### Validation

- Run the targeted Pingora test binary and the normal checks required by that
  repository.
- Run the targeted Edgion DSL integration scenario when the environment is
  available.
- Run Edgion's required pre-commit checks in `AGENTS.md`.
- Run `python3 cicd/checks/check_pingora_source_policy.py` after updating the
  fork revision.
- Do not run Edgion `cargo test --all` unless explicitly requested.

## Repository and sequencing constraints

- Do not commit or push either repository without explicit user authorization.
- The Pingora fork change must receive a commit before Edgion can pin it in
  `Cargo.lock`.
- Before that commit exists, Edgion may be tested against the local fork through
  a temporary Cargo source patch, but no machine-specific path may remain in
  committed project configuration.
- Preserve unrelated worktree changes in both repositories.
- The originating Edgion worktree contained unrelated edits in
  `pg_fail_to_proxy.rs`; implementations must inspect and preserve them rather
  than replacing the file wholesale.

## Acceptance criteria

- The design classifies this as a performance feature and explicitly defines
  the expanded Pingora reuse contract.
- A complete response-stage local reply can be followed by a second request on
  the same HTTP/1 connection.
- A codec-level keepalive veto still closes the connection even if
  `can_reuse_downstream` is true.
- The wire never advertises keepalive followed by an unavoidable post-response
  close.
- The local reply remains single-write, and no replaced upstream response byte
  reaches the downstream.
- HTTP/2 behavior remains unchanged.
- Pingora and Edgion regression tests cover the behavior at their respective
  boundaries.
- Edgion pins one common new commit for the complete Pingora fork dependency
  family.
- Review knowledge records the final implemented or declined decision.

## Document index

| Document | Status | Notes |
|----------|--------|-------|
| `00-context.md` | done | Shortest knowledge and code-entry map |
| `01-design.md` | not created | Produce during brainstorming/design |
| `02-implementation.md` | not created | Produce after design approval |
| `03-subtasks.md` | not created | Required if the accepted design remains cross-repository |
| `04-todo.md` | not created | Create only for scattered follow-ups |
| `05-issues.md` | not created | Create when a blocker is confirmed |
| `06-decisions.md` | not created | Record the final lifecycle/API decision |
| `07-changelog.md` | not created | Optional for this task |

## Decision log

| Date | Decision | Reason |
|------|----------|--------|
| 2026-08-03 | Reclassified from a review fix to a P3 performance feature | Current close behavior is safe and accurately framed; reuse is an additional capability |
| 2026-08-03 | Require dedicated brainstorming before implementation | The change expands a Pingora core lifecycle contract across repositories |
