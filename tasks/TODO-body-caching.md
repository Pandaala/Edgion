# TODO: Request Body Caching Support

Tracking: [cloudflare/pingora#816](https://github.com/cloudflare/pingora/pull/816)

## Blocked on

- [ ] PR #816 merged into Pingora upstream

## Once PR #816 is merged

- [ ] Upgrade Pingora dependency to the version containing PR #816
- [ ] Implement `early_request_body_buffer_limit()` in `EdgionHttpProxy`
  - Route-level config: `body_buffer_max_size` (default 10MB)
  - Only enable when matched route has plugins that declare `needs_request_body()`
- [ ] Add `needs_request_body()` to `RequestFilter` trait (default `false`)
- [ ] Extend `PluginSession` trait with `get_request_body()` / `set_request_body()`
- [ ] Implement `upstream_request_filter()` to sync `Content-Length` when body is modified
- [ ] Build first body-aware plugin (WAF or body-sign)
- [ ] Tests: large body 413, empty body, chunked, H2, body modification + Content-Length sync
