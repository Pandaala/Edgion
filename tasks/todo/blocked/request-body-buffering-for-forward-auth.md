# Request Body Buffering — ForwardAuth Body Forwarding Support

> The ForwardAuth plugin currently does not support forwarding the request body to the authentication service.
> A body buffering mechanism is needed so the body can be sent to the auth service first for validation,
> and then forwarded to the upstream only after authentication succeeds.

## Background

### Current Limitation

Edgion's ForwardAuth plugin forwards headers only, not the body:

| Feature | Edgion | Traefik | APISIX | nginx |
|---------|--------|---------|--------|-------|
| Forward body | ❌ | ✅ `forwardBody` | ❌ | ❌ |

The current implementation sends the auth request with reqwest during the `run_request` phase, without a body:

```rust
// src/core/gateway/plugins/http/forward_auth/plugin.rs L214
// Send auth request (no body)
let resp = client.request(method, &self.config.uri)
    .headers(auth_headers)
    .timeout(timeout)
    .send()
    .await;
```

### Use Cases

- The authentication service needs to inspect the request body content, such as signature verification or payload validation
- Webhook validation scenarios where the body is required to compute an HMAC signature
- API request auditing where the auth service needs the full request content for compliance checks

### Why Not Use Pingora Pipe Subrequest

The pipe subrequest API introduced in Pingora 0.8.0 provides a `SavedBody` mechanism that can capture and reuse the body,
but it is not suitable for this scenario after evaluation (see `tasks/working/pingora-0.8.0-upgrade/09-pipe-subrequests.md`):

1. **Call-layer mismatch**: `pipe_subrequest` requires Pingora `&mut Session`, while plugins run on the `PluginSession` abstraction layer
2. **Upstream becomes a subrequest**: once the body is consumed by the pipe, the normal proxy flow can no longer read it, so the upstream must also be handled through a subrequest
3. **Plugin recursion**: the subrequest re-executes all plugins, causing ForwardAuth to recurse infinitely
4. **Unstable API**: pipe subrequest is explicitly marked alpha, and the API may change at any time

## Design

### Core Idea: Body Buffer + Delayed Forwarding

Buffer the body during the `request_body_filter` phase, then release it to the upstream after auth completes.

### Data Flow

```
Client → [body chunk 1] → request_body_filter
                            ↓
                          ctx.body_buffer exists?
                            ↓ Yes
                          append to buffer, suppress chunk (do not send upstream)
                            ↓
         [body chunk N] → end_of_stream = true
                            ↓
                          reqwest POST auth_uri (with full body)
                            ↓
                          auth returns 2xx?
                         /            \
                       Yes             No
                        ↓               ↓
                release buffer      return error response
                send to upstream    terminate request
```

### Required Changes

1. **ForwardAuthConfig**: add new configuration fields:
   - `forward_body: bool` — whether to forward the body (default `false`)
   - `max_body_size: usize` — body buffering size limit (default 1 MB)

2. **EdgionHttpContext**: add body buffer state:
   - Follow a pattern similar to `MirrorState`, maintaining a buffer state machine in the context
   - States: `Buffering` → `AuthPending` → `Releasing` / `Rejected`

3. **ForwardAuth plugin `run_request`**: initialize the body buffer in the context when `forward_body: true`

4. **`pg_request_body_filter`**: detect and handle the body buffer state:
   - `Buffering`: buffer the chunk and suppress forwarding
   - `Releasing`: release buffered chunks to the upstream
   - Return `413` if the body exceeds `max_body_size`

5. **Auth trigger timing**: trigger the auth call inside the body filter after the full body has been read

### Key Constraints

- Body buffering consumes memory, so a strict size limit is required
- The auth call blocks body forwarding to the upstream, increasing request latency
- This feature is not suitable for large file uploads
- Existing behavior remains unchanged when `forward_body: false` (default)

## Files Involved

- `src/core/gateway/plugins/http/forward_auth/plugin.rs`
- `src/core/gateway/routes/http/proxy_http/pg_request_body_filter.rs`
- `src/types/ctx.rs` — body buffer state
- `src/types/resources/edgion_plugins/plugin_configs/forward_auth.rs` — configuration fields

## Priority

P3 — Feature enhancement, non-blocking requirement

## Action Items

- [ ] Design the body buffer state machine (refer to the `MirrorState` pattern)
- [ ] Add `forward_body` / `max_body_size` to `ForwardAuthConfig`
- [ ] Implement buffering logic in `pg_request_body_filter`
- [ ] Add support in the ForwardAuth plugin for sending auth requests with the body
- [ ] Add size limit protection for oversized bodies (`413` response)
- [ ] Add integration tests
- [ ] Update documentation
