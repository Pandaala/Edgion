# Edgion 扩展过滤器

Edgion 提供的 HTTP Route 级别插件，通过 `EdgionPlugins` CRD 配置。

## 认证保护域 {#authentication-realm}

BasicAuth、KeyAuth、JwtAuth、HmacAuth、LdapAuth 和 OpenidConnect 的 `config.realm`
使用统一的字符规则。填写原始保护域名称，Edgion 会在写入 `WWW-Authenticate` 时
转义双引号和反斜杠。仅按 YAML/JSON 的语法转义，不要预先做 HTTP 转义。

禁止 ASCII 控制字符（`0x00`–`0x1F`，包括 CR、LF、NUL 和制表符）及 DEL（`0x7F`）；
这些字符会使配置校验失败，不会被静默删除。其他字符保留原值，各插件现有默认值及空值规则不变。

例如，YAML `realm: 'Partner "API"\'` 对应响应中的参数 `realm="Partner \"API\"\\"`。

启用的插件配置了非法 realm 时，`EdgionPlugins` 会报告 `Accepted=False`。
Gateway 对经过该资源的请求返回 HTTP 500，包括热更新为非法值的情况；改为合法值后恢复正常认证。
校验消息只说明字符规则，不回显 realm 内容。禁用的插件条目不参与校验和执行。

## 内容

| 文档 | 说明 |
|------|------|
| [ai-guard.md](ai-guard.md) | 请求侧提示词守护：regex/PII 规则、掩码、拒绝，以及一个有界外部评估服务 |
| [ai-proxy.md](ai-proxy.md) | Rule 级 AiProxy 入口；完整文档见 [AI Gateway](../../../../ai-gateway/) |
| [bandwidth-limit.md](bandwidth-limit.md) | 带宽限制 |
| [basic-auth.md](basic-auth.md) | HTTP Basic 认证 |
| [canary.md](canary.md) | 全量切换、可远程切换的金丝雀流量插件 |
| [conditions.md](conditions.md) | 插件 `run`/`skip` 条件（`keyExist`、`keyMatch`、`timeRange`、`probability`） |
| [cors.md](cors.md) | 跨域资源共享（CORS） |
| [csrf.md](csrf.md) | CSRF 防护 |
| [credential-injector.md](credential-injector.md) | 从 Secret 或 LinkSys 注入上游 Bearer 凭据或自定义 Header |
| [ctx-setter.md](ctx-setter.md) | 上下文变量提取 |
| [webhook-key-get.md](webhook-key-get.md) | 将 LinkSys webhook 的值解析进请求上下文 |
| [direct-endpoint.md](direct-endpoint.md) | 直连指定 Endpoint |
| [dsl/README.md](dsl/README.md) | DSL 脚本引擎 |
| [dye.md](dye.md) | 流量标记请求头（条目级 `dye`——结果词表、value 模板、防伪造剥离） |
| [dynamic-upstream.md](dynamic-upstream.md) | 动态上游选择 |
| [forward-auth.md](forward-auth.md) | 外部认证委托 |
| [form-json-transform.md](form-json-transform.md) | 在扁平 Form URL 编码与 JSON 之间转换有界请求体 |
| [ext-proc.md](ext-proc.md) | 经 gRPC 的外部处理（兼容 Envoy ext_proc） |
| [request-access-policy.md](request-access-policy.md) | 七层 host/path/IP 访问策略，支持 Secret/HMAC 条件和 profile 热切换 |
| [grpc-json-transcoder.md](grpc-json-transcoder.md) | 基于 descriptor 的 REST JSON 到原生 gRPC 转码，上游强制 HTTP/2 |
| [graphql-operation-router.md](graphql-operation-router.md) | 按操作名称选择 HTTPRoute 已声明的 Service 后端 |
| [graphql-rate-limit.md](graphql-rate-limit.md) | 静态 SDL 查询成本与 Redis 预算限流 |
| [header-cert-auth.md](header-cert-auth.md) | 请求头/证书认证 |
| [hmac-auth.md](hmac-auth.md) | HMAC 签名验证 |
| [ip-restriction.md](ip-restriction.md) | IP 黑白名单 |
| [jwe-decrypt.md](jwe-decrypt.md) | JWE Token 解密 |
| [json-schema-validation.md](json-schema-validation.md) | 使用内联 JSON Schema 校验 JSON 请求体 |
| [jwt-auth.md](jwt-auth.md) | JWT 认证 |
| [key-auth.md](key-auth.md) | API Key 认证 |
| [ldap-auth.md](ldap-auth.md) | LDAP 认证 |
| [mock.md](mock.md) | Mock 响应 |
| [openid-connect.md](openid-connect.md) | OpenID Connect 认证 |
| [proxy-rewrite.md](proxy-rewrite.md) | 代理重写 |
| [rate-limit-local.md](rate-limit-local.md) | 限流（内存，近似） |
| [rate-limit-redis.md](rate-limit-redis.md) | 限流（Redis，精确，集群级） |
| [real-ip.md](real-ip.md) | 获取客户端真实 IP |
| [region-route.md](region-route.md) | 区域路由 |
| [request-mirror.md](request-mirror.md) | 请求镜像 |
| [request-body-buffer.md](request-body-buffer.md) | 显式缓存请求 body，供后续阶段消费 |
| [request-restriction.md](request-restriction.md) | 请求过滤 |
| [response-rewrite.md](response-rewrite.md) | 响应重写 |
| [request-id.md](request-id.md) | RequestId 与默认 W3C 上下文 |
| [wasm.md](wasm.md) | 通过 wasmtime 执行 proxy-wasm ABI guest 模块（WebAssembly） |
| [waf.md](waf.md) | 原生 WAF、请求体前缀准入与完整检查 |
| [all-endpoint-status.md](all-endpoint-status.md) | 查询所有后端 Endpoint 状态 |
| [debug-access-log.md](debug-access-log.md) | 将 Access Log 注入响应头（仅开发用） |
