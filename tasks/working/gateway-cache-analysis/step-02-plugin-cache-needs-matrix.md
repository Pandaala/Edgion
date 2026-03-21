# Step 02 - Plugin Cache Needs Matrix

## Cache Type Taxonomy

先统一术语，后面每个插件都按这些类型归类：

- `material-cache`
  - 把 Secret、证书、公钥、用户表、key 表等运行材料常驻内存
- `auth-result-cache`
  - 缓存一次认证/鉴权结果，降低远端调用或高成本验签
- `compiled-artifact-cache`
  - 缓存 regex、bytecode、matcher、预解析结果
- `local-state-cache`
  - 单节点运行态计数/统计/近似算法状态
- `distributed-state-cache`
  - 借助 Redis/Etcd 等跨节点共享状态
- `content-cache`
  - 上游响应对象缓存
- `request-body-cache`
  - 为重读、验签、重放、异步处理保留请求体
- `short-ttl-result-cache`
  - 对 fan-out / 查询类插件缓存短时结果，降后端压力

## A. 已经明确使用 cache 的插件

| 插件 | 当前状态 | cache 类型 | 代码依据 | 说明 |
|---|---|---|---|---|
| `BasicAuth` | 已实现 | `material-cache`, `auth-result-cache` | `src/core/gateway/plugins/http/basic_auth/plugin.rs:28-36`, `:116-131`, `:168-213` | 用户表常驻内存；成功认证 header 做 5 分钟正缓存 |
| `Cors` | 已实现 | `compiled-artifact-cache` | `src/types/resources/edgion_plugins/plugin_configs/cors.rs:56-70`, `:130-206` | exact origin HashMap + compiled regex；另有浏览器侧 `maxAge` 语义 |
| `JwtAuth` | 已实现 | `material-cache` | `src/core/gateway/plugins/http/jwt_auth/plugin.rs:64-70`, `:131-156` | 首次请求懒加载 Secret / key material 并驻留 |
| `LdapAuth` | 已实现 | `auth-result-cache` | `src/core/gateway/plugins/http/ldap_auth/plugin.rs:20-25`, `:101-135`, `:263-280` | 对 `username:password` 成功 bind 做 TTL 正缓存 |
| `OpenidConnect` | 已实现 | `material-cache`, `auth-result-cache`, `short-ttl-result-cache`, `singleflight-cache` | `src/core/gateway/plugins/http/openid_connect/plugin.rs:125-138`; `src/core/gateway/plugins/http/openid_connect/openid_impl.rs:1102-1195` | discovery/JWKS/introspection/access token/refresh result 都有本地缓存与并发抑制 |
| `RateLimit` | 已实现 | `local-state-cache` | `src/core/gateway/plugins/http/rate_limit/plugin.rs:56-65`, `:91-107` | 每个插件实例持有本地 CMS rate estimator |
| `RateLimitRedis` | 已实现 | `distributed-state-cache`, `compiled-artifact-cache` | `src/core/gateway/plugins/http/rate_limit_redis/plugin.rs:119-145`, `:172-200` | 限流状态在 Redis；Lua SHA 用 `OnceCell` 缓存 |
| `CtxSet` | 已实现 | `compiled-artifact-cache` | `src/types/resources/edgion_plugins/plugin_configs/ctx_set.rs` | 规则 regex 预编译 |
| `DirectEndpoint` | 已实现 | `compiled-artifact-cache` | `src/types/resources/edgion_plugins/plugin_configs/direct_endpoint.rs` | 路由提取 regex 预编译 |
| `DynamicInternalUpstream` | 已实现 | `compiled-artifact-cache` | `src/types/resources/edgion_plugins/plugin_configs/dynamic_internal_upstream.rs` | target 提取 regex 预编译 |
| `DynamicExternalUpstream` | 已实现 | `compiled-artifact-cache` | `src/types/resources/edgion_plugins/plugin_configs/dynamic_external_upstream.rs` | target/domain 提取 regex 预编译 |
| `ProxyRewrite` | 已实现 | `compiled-artifact-cache` | `src/types/resources/edgion_plugins/plugin_configs/proxy_rewrite.rs` | rewrite regex 预编译 |
| `RequestRestriction` | 已实现 | `compiled-artifact-cache` | `src/types/resources/edgion_plugins/plugin_configs/request_restriction.rs:12-15`, `:123-142` | 大集合转 HashSet，regex 合并编译 |
| `Dsl` | 已实现 | `compiled-artifact-cache` | `src/core/gateway/plugins/http/dsl/config.rs:70-80`, `:182-205` | 配置校验阶段缓存编译后的 bytecode |

## B. 已有“材料常驻内存”，但还不是成熟 cache 框架的插件

| 插件 | 当前状态 | cache 需求判断 | 说明 |
|---|---|---|---|
| `KeyAuth` | 部分具备 | `material-cache` | key -> metadata 基本就是常驻内存查表，是否再加 TTL cache 取决于后续是否引入外部 key provider |
| `HmacAuth` | 部分具备 | `material-cache` | credential map 适合常驻；若未来支持 body 验签，则还需要 `request-body-cache` |
| `HeaderCertAuth` | 部分具备 | `material-cache` | 证书/映射材料可内存驻留；通常不需要结果缓存 |
| `JweDecrypt` | 部分具备 | `material-cache` | 解密密钥已用 `RwLock` 持有，属于材料缓存 |
| `IpRestriction` | 部分具备 | `compiled-artifact-cache` | 如果 CIDR/网段集合很大，适合统一做 matcher 预编译 |
| `RealIp` | 部分具备 | `compiled-artifact-cache` | trusted proxy/CIDR 也更适合预构建 matcher，而不是请求时临时判断 |

## C. 明显值得补 cache 的插件

| 插件 | 建议 cache 类型 | 原因 | 建议优先级 |
|---|---|---|---|
| `ForwardAuth` | `auth-result-cache`, `singleflight-cache` | 外部 auth 服务延迟高、易放大；对同 token / session 可做短 TTL 正缓存 | 高 |
| `AllEndpointStatus` | `short-ttl-result-cache` | 当前会 fan-out 后端查询；如果用于状态页或探测接口，短 TTL 能显著降压 | 中 |
| `DynamicExternalUpstream` | `short-ttl-result-cache` | 若后续引入外部发现 / DNS / metadata 查询，建议增加短 TTL 解析结果缓存 | 中 |
| `RequestMirror` | `short-ttl-result-cache` 不是核心，`buffer/spool` 更重要 | 更像异步复制，不是典型 cache 场景；若需要重试或延迟发送，可能需要 body/cache spool | 低 |

## D. 暂时不需要额外 cache 的插件

这些插件当前更偏“纯变换”或“纯判断”，没有明显独立 cache 价值：

- `RequestHeaderModifier`
- `ResponseHeaderModifier`
- `RequestRedirect`
- `UrlRewrite`
- `Mock`
- `DebugAccessLogToHeader`
- `ResponseRewrite`
- `BandwidthLimit`
- `Csrf`

说明：

- 它们可能依赖 route/runtime 的上层 cache，但自身通常不需要再持有专门 cache
- 若后续功能变复杂，比如 `Csrf` 增加服务端 nonce 存储，才会出现真正状态 cache 需求

## E. 明确存在“缺少 request body cache”的地方

最明确的信号已经出现在 `HmacAuth` 配置校验里：

- `validateRequestBody` 目前直接报错，原因就是“需要 request body cache support”
  - `src/types/resources/edgion_plugins/plugin_configs/hmac_auth.rs:230-231`

这意味着一旦 Edgion 想支持以下能力，就不能只靠 header 级插件接口：

- body 签名验签
- body hash / digest
- 请求体重放
- 鉴权后把 body 再转发给上游
- 大 body 的分片/磁盘临存

这类需求不应混入普通 key-value cache，而应单独设计：

- 小体积内存 buffer
- 大体积 spill-to-disk
- 可重复读取句柄
- 上限、超时、背压、清理策略

## F. Stream 插件的 cache 需求

当前 stream 插件总体更轻：

- `stream/ip_restriction`
- `stream/tls_route/ip_restriction`

判断：

- 更偏 `compiled-artifact-cache`，比如 CIDR / matcher 预解析
- 不适合引入内容缓存
- 通常也不需要认证结果 cache

## G. 总结性判断

从插件视角看，Edgion 真正需要的不是一种 cache，而是五种核心能力：

1. 材料缓存：Secret、公钥、用户表、密钥表
2. 认证结果缓存：BasicAuth / LDAP / OIDC / ForwardAuth
3. 编译产物缓存：regex、DSL bytecode、条件 matcher
4. 本地状态缓存：RateLimit、LB 统计
5. 分布式状态缓存：RateLimitRedis 这类跨节点一致性场景

另外还缺一块“基础设施型能力”：

6. request body cache / spool

它不是单个插件的局部优化，而是未来一批安全类和变换类插件的前置能力。

## Need Confirmation

- 如果未来要做 `EdgionCache`，它更适合作为“内容缓存插件”，不应直接承载 `RateLimitRedis` 这类共享状态
- 如果未来要做统一 plugin cache API，建议拆成：
  - `local_ttl_cache`
  - `shared_state_store`
  - `request_body_store`

## Risks

- 把 `ForwardAuth` 的结果缓存做成过长 TTL，会放大权限撤销延迟
- 对 `BasicAuth` / `LDAP` / `OIDC` 的缓存若只清材料不清结果，会产生脏授权
- 对 `RateLimit` 这类状态型插件误用内容缓存语义，会导致容量与正确性完全失配

