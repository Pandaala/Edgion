# Pingora 使用注意事项

## Hostname 提取

**结论：在 HTTP/2 + HTTPS 场景下，Pingora 将 hostname 放在了 URI 中，而不是作为独立的 header。**

提取 hostname 的正确方式（按优先级）：

```rust
let hostname = req_header.uri.host()                    // HTTP/2 (HTTPS)
    .or_else(|| req_header.headers.get("host"))         // HTTP/1.1
    .or_else(|| req_header.headers.get(":authority"));  // HTTP/2 fallback
```

参考: `src/core/routes/http_routes/edgion_http_pingora.rs:request_filter()`

