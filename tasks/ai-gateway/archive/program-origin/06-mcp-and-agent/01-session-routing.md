# AI-6/01 - MCP 会话路由

**优先级**: P0 —— MCP 特有，最容易漏
**依赖**: 无（模式 A 的能力已具备）
**来源**: 原 `mcp/02-session-affinity.md`，新增模式 B

## 问题

MCP server 是**有状态**的：`initialize` 之后的 session 状态（已协商能力、已注册 tool、
订阅关系）保存在**某个后端实例的内存里**。

如果网关不做会话路由：

- 客户端 POST 打到实例 A、GET 开流打到实例 B → 会话不存在，直接 404 / `Invalid session ID`
- 后端多副本时表现为**随机失败**，副本越多失败率越高，极难排查

旧版 HTTP+SSE 传输同理，session 标识在 URL query 的 `sessionId` 上。

## 两种路由模式

网关应当**两种都支持**，按后端形态选择。

### 模式 A：一致性哈希（能力已具备）

按 header 做一致性哈希**已经端到端打通**，不需要新写 LB 策略：

```
EdgionBackendTrafficPolicy.loadBalancer.consistentHash{ hashOn: Header, key: "..." }
  ↓ edgion-resources/src/resources/edgion_backend_traffic_policy.rs:327
ConsistentHashOn::Header(name)
  ↓ edgion-gateway/src/backends/selector.rs:150  extract_hash_key()
hash_key: Vec<u8>
  ↓ edgion-gateway/src/lb/compiled_selector.rs:239
edgion-gateway/src/lb/selection/consistent_hash.rs:61  select()
```

配置上现在就可以写：

```yaml
loadBalancer:
  consistentHash:
    hashOn: Header
    key: Mcp-Session-Id
```

- 优点：零开发，网关完全无状态，后端无感
- 缺点：路由是**推断**出来的。后端实例增减会引起部分会话重映射；
  且网关无法知道某个 session 究竟属于哪个实例，只能靠哈希碰运气碰对

### 模式 B：session-id 编码（新增）

网关拦下 `initialize` 响应，把**后端标识**编码进下发给客户端的 `Mcp-Session-Id`，
后续请求解码即知目标，不依赖哈希环稳定性。

```
客户端 initialize
  → 网关按普通 LB 选实例 B3，转发
  → 后端返回 Mcp-Session-Id: srv-abc123
  → 网关改写为 Mcp-Session-Id: <编码(B3) + srv-abc123>，下发客户端
  → 后续请求：网关解码得到 B3，直接路由；把原始 id 还原后转发给后端
```

- 优点：路由是**确定**的，扩缩容不影响已有会话；任意网关副本都能解码，网关仍然无状态
- 缺点：需要改写响应头和请求头；编码必须**防伪造**（否则客户端可以构造 session id
  指定打到任意后端 —— 这是一个 SSRF 风险面）
- Envoy AI Gateway 的 MCP Gateway 用的就是这个手段（它编码的是多个后端的 session id，
  用于 tool 聚合；这里只用单后端定位这一部分）

实现要点：

- [ ] 编码内容至少包含后端标识；用带密钥的 MAC 签名，网关侧验签失败即拒绝
- [ ] 密钥来源与轮换策略（多网关副本必须共享同一密钥）
- [ ] 编码后的 id 长度要控制（部分客户端对 header 长度敏感）
- [ ] 目标后端已下线时的行为：明确报"会话失效"让客户端重新 `initialize`，
      而不是静默路由到别的实例（那会返回难以理解的错误）

### 模式选择

| 场景 | 建议 |
|------|------|
| 后端副本数稳定、可接受偶发会话失效 | 模式 A（零成本） |
| 后端频繁扩缩容、要求会话稳定 | 模式 B |
| 旧版 HTTP+SSE 双端点传输 | 模式 A + `hashOn: QueryParam, key: sessionId`（模式 B 在 GET 阶段还没有 session id，用不了） |

## 需要实现

### 1. 空 key 边界（模式 A 的关键）

MCP 的第一个请求是 `initialize`，此时**客户端还没有 session id**，`Mcp-Session-Id`
头不存在 → `extract_hash_key` 返回空。

- [ ] 确认 `compiled_selector.rs:239` 附近注释所说的"empty key falls back"具体回退到
      什么行为（round-robin？固定节点？）
- [ ] 明确期望语义：**空 key 应回退到均衡分发**（initialize 请求可以落到任意实例，
      落到哪个实例就由该实例下发 session id，后续请求自然被路由到同一实例）
- [ ] 如果当前回退是"所有空 key 打到同一个节点"，必须修 —— 那会让所有 initialize
      请求打爆单个实例
- [ ] 为空 key 的回退路径加计数指标，便于观察 initialize 占比

### 2. header 名大小写

- [ ] 确认 `extract_hash_key` 取 header 时是大小写不敏感的（HTTP 头本身大小写不敏感，
      客户端可能发 `mcp-session-id`）

### 3. 模式 B 实现

- [ ] 响应头改写钩子：`pg_response_filter` 处拦截 `initialize` 响应的 `Mcp-Session-Id`
- [ ] 请求侧解码 + 还原原始 id 后转发
- [ ] MAC 签名与密钥管理
- [ ] 解码失败 / 后端已下线的错误语义

### 4. 后端伸缩时的稳定性

- [ ] 模式 A：确认后端实例增减时一致性哈希的重映射比例可接受
      （ketama ring 本身满足，确认虚拟节点数配置合理）
- [ ] **两种模式的共同限制**：实例被摘除时该实例上的 MCP 会话必然丢失，
      这是有状态服务的固有属性，网关做不了。需要在文档里写清楚，
      让用户知道后端需要自己做会话持久化才能无损滚动

### 5. 文档与示例

- [ ] 给出 MCP 后端的完整配置示例（HTTPRoute + EdgionBackendTrafficPolicy）
- [ ] 说明两种模式的取舍与旧版传输的写法

## 测试

- [ ] 3 个后端实例，模拟 MCP 流程：POST initialize → 记录返回的 session id 与实例 →
      后续 10 个带该 session id 的请求全部落到同一实例（两种模式各测一遍）
- [ ] 100 个不同 session id 在 3 个实例上的分布均匀性
- [ ] 无 `Mcp-Session-Id` 头的请求（initialize）在实例间均衡分发，不集中到单点
- [ ] header 大小写混写（`mcp-session-id` / `MCP-Session-Id`）路由到同一实例
- [ ] 摘除一个实例后，其余 session 的映射保持不变
- [ ] **模式 B**：扩容后（实例数从 3 变 5）已有会话全部仍路由到原实例
- [ ] **模式 B**：伪造/篡改的 session id 被验签拒绝，不产生任意后端路由
