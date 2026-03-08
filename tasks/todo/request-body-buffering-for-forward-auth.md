# Request Body Buffering — ForwardAuth Body 转发支持

> ForwardAuth 插件当前不支持将请求 body 转发到认证服务。需要实现 body buffering 机制，
> 使 body 可以先发给 auth 服务验证，验证通过后再转发给 upstream。

## 背景

### 当前限制

Edgion 的 ForwardAuth 插件只转发 headers，不转发 body：

| 特性 | Edgion | Traefik | APISIX | nginx |
|------|--------|---------|--------|-------|
| 转发 Body | ❌ | ✅ `forwardBody` | ❌ | ❌ |

当前实现在 `run_request` 阶段用 reqwest 发送认证请求，无 body：

```rust
// src/core/gateway/plugins/http/forward_auth/plugin.rs L214
// Send auth request (no body)
let resp = client.request(method, &self.config.uri)
    .headers(auth_headers)
    .timeout(timeout)
    .send()
    .await;
```

### 使用场景

- 认证服务需要检查请求 body 内容（如签名验证、payload 校验）
- Webhook 验证场景：需要 body 计算 HMAC 签名
- API 请求审计：auth 服务需要完整请求内容做合规检查

### 为什么不用 Pingora Pipe Subrequest

Pingora 0.8.0 新增的 pipe subrequest API 提供了 `SavedBody` 机制可以捕获和复用 body，
但经评估不适合此场景（详见 `tasks/working/pingora-0.8.0-upgrade/09-pipe-subrequests.md`）：

1. **调用层级不匹配**：pipe_subrequest 需要 Pingora `&mut Session`，插件运行在 `PluginSession` 抽象层
2. **Upstream 变成 subrequest**：body 被 pipe 消费后，正常 proxy 流程读不到 body，upstream 也必须用 subrequest 处理
3. **插件递归**：subrequest 重新执行所有插件，ForwardAuth 会无限递归
4. **API 不稳定**：pipe subrequest 明确标注为 alpha，API 随时可能变更

## 设计方案

### 核心思路：Body Buffer + 延迟转发

在 `request_body_filter` 阶段缓存 body，auth 完成后释放给 upstream。

### 数据流

```
Client → [body chunk 1] → request_body_filter
                            ↓
                          ctx.body_buffer 存在？
                            ↓ Yes
                          追加到 buffer，抑制 chunk（不发 upstream）
                            ↓
         [body chunk N] → end_of_stream = true
                            ↓
                          reqwest POST auth_uri (带完整 body)
                            ↓
                          auth 返回 2xx？
                         /          \
                       Yes           No
                        ↓             ↓
                  释放 buffer      返回错误 response
                  发给 upstream    终止请求
```

### 涉及的改动

1. **ForwardAuthConfig** — 新增配置项：
   - `forward_body: bool` — 是否转发 body（默认 false）
   - `max_body_size: usize` — body 缓存上限（默认 1MB）

2. **EdgionHttpContext** — 新增 body buffer 状态：
   - 类似 `MirrorState` 的模式，在 ctx 中维护 buffer 状态机
   - 状态：`Buffering` → `AuthPending` → `Releasing` / `Rejected`

3. **ForwardAuth plugin `run_request`** — 如果 `forward_body: true`，在 ctx 中初始化 body buffer

4. **`pg_request_body_filter`** — 检测 body buffer 状态：
   - `Buffering`：缓存 chunk，抑制发送
   - `Releasing`：释放缓存的 chunks 给 upstream
   - body 超过 `max_body_size` 时返回 413

5. **Auth 调用时机** — body 全部读完后，在 body filter 中触发 auth 调用

### 关键约束

- Body buffer 占用内存，必须有大小上限
- Auth 调用阻塞了 body 到 upstream 的转发，增加了请求延迟
- 大文件上传场景不适合开启此功能
- `forward_body: false`（默认）时不影响现有行为

## 涉及文件

- `src/core/gateway/plugins/http/forward_auth/plugin.rs`
- `src/core/gateway/routes/http/proxy_http/pg_request_body_filter.rs`
- `src/types/ctx.rs` — body buffer 状态
- `src/types/resources/edgion_plugins/plugin_configs/forward_auth.rs` — 配置项

## 优先级

P3 — 功能增强，非阻塞性需求

## 行动项

- [ ] 设计 body buffer 状态机（参考 MirrorState 模式）
- [ ] ForwardAuthConfig 新增 `forward_body` / `max_body_size` 配置
- [ ] 实现 `pg_request_body_filter` 中的 buffer 逻辑
- [ ] ForwardAuth plugin 支持带 body 发送认证请求
- [ ] 添加 body 超限保护（413 response）
- [ ] 集成测试
- [ ] 更新文档
