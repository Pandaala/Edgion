# AI-6/02 - MCP OAuth 2.1 资源服务器

**优先级**: P1
**依赖**: 无（可与 01 并行）
**预估**: 大
**来源**: 原 `mcp/04-mcp-oauth-resource-server.md`

## 问题

MCP 规范定义的授权模型是 **OAuth 2.1 + RFC 9728 Protected Resource Metadata**。
MCP client（Claude Desktop、IDE 插件等）会按标准流程发现授权服务器并获取 token。

如果每个后端 MCP server 各自实现 OAuth，成本高且容易做错。网关是做这一层的天然位置：
**后端只管业务，认证收敛到网关**。

## 协议流程

```
1. Client 无 token 请求 → 网关 401
   WWW-Authenticate: Bearer resource_metadata="https://gw.example.com/.well-known/oauth-protected-resource"

2. Client GET /.well-known/oauth-protected-resource
   ← { "resource": "https://gw.example.com/mcp",
       "authorization_servers": ["https://auth.example.com"] }

3. Client 从 authorization server 走标准 OAuth 2.1 流程（含 PKCE）拿到 token
   （这一步不经过网关）

4. Client 带 Authorization: Bearer <token> 重新请求 → 网关校验后放行
```

规范要求的关键点：

- **audience 绑定**：token 的 `aud` 必须匹配本资源标识，防止 token 被跨服务重放
- **必须用 PKCE**（由 client 与 authorization server 保证，网关不参与）
- token **不得**透传给上游（除非显式配置），网关校验后应剥离或替换

## 现状

- 已有 `edgion-gateway/src/plugins/http/jwt_auth/` —— JWT 校验能力可复用
  （签名验证、claims 提取、JWKS 拉取）
- 已有 `edgion-gateway/src/plugins/http/key_auth/`
- 已有 `edgion-gateway/src/plugins/http/openid_connect/`（含 `jwks_provider.rs`、
  `jwt_common.rs`），JWKS 拉取与缓存逻辑应从这里复用而不是重写
- 没有 OAuth resource metadata 相关实现

## 需要实现

### 1. Protected Resource Metadata 端点

- [ ] 网关内置 `/.well-known/oauth-protected-resource` 响应（参考现有 `direct_endpoint` /
      `all_endpoint_status` 插件的内置端点模式）
- [ ] 支持按 path 前缀的 metadata（RFC 9728 允许 `/.well-known/oauth-protected-resource/mcp` 形式）
- [ ] 内容来自配置：`resource`、`authorization_servers`、`scopes_supported`、
      `bearer_methods_supported`

### 2. Bearer token 校验

- [ ] 新插件 `mcp_auth`（或扩展 `jwt_auth` 增加 MCP 模式）
- [ ] 校验签名（复用 `openid_connect/jwks_provider.rs` 的 JWKS 拉取 + 缓存 + 轮换）
- [ ] 校验 `aud` 与配置的 resource 标识一致 —— **不可省略**，这是规范的核心安全要求
- [ ] 校验 `exp` / `nbf` / `iss`
- [ ] 校验 scope（可选，按 method 粒度的 scope 检查留到 [03-jsonrpc-governance.md](03-jsonrpc-governance.md)）

### 3. 401 挑战响应

- [ ] 无 token / token 无效时返回 401，且 `WWW-Authenticate` 头必须携带
      `resource_metadata` 参数指向元数据端点 —— MCP client 靠这个自动发现授权服务器，
      缺失会导致客户端无法完成授权引导
- [ ] 区分 401（未认证）与 403（已认证但无权限）

### 4. token 传递策略

- [ ] 默认**不**向上游透传 `Authorization` 头
- [ ] 可配置：透传原 token / 替换为内部凭证 / 注入身份信息头（如 `X-Mcp-Subject`）
- [ ] 若注入身份头，必须先剥离客户端伪造的同名头

## 配置形态草案

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
spec:
  requestPlugins:
    - type: McpAuth
      config:
        resource: "https://gw.example.com/mcp"
        authorizationServers:
          - "https://auth.example.com"
        jwksUri: "https://auth.example.com/.well-known/jwks.json"
        scopesSupported: ["mcp:read", "mcp:tools"]
        forwardToken: false
```

## 测试

- [ ] 无 token → 401 且 `WWW-Authenticate` 含正确的 `resource_metadata` URL
- [ ] metadata 端点返回符合 RFC 9728 的 JSON
- [ ] `aud` 不匹配的 token → 401（重点用例，覆盖跨服务重放）
- [ ] 过期 token → 401
- [ ] 有效 token → 放行，且上游收不到 `Authorization` 头（默认配置下）
- [ ] JWKS 轮换后旧 token 失效、新 token 可用
- [ ] 客户端伪造 `X-Mcp-Subject` 头被剥离

## 备注

范围较大，可拆两步交付：先做 **2（token 校验）+ 3（401 挑战）**，metadata 端点先用
静态配置托管；之后再补完整的动态 metadata。
