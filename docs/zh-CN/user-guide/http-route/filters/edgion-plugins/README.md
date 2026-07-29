# Edgion 扩展过滤器

Edgion 提供的 HTTP Route 级别插件，通过 `EdgionPlugins` CRD 配置。

## 内容

| 文档 | 说明 |
|------|------|
| [bandwidth-limit.md](bandwidth-limit.md) | 带宽限制 |
| [basic-auth.md](basic-auth.md) | HTTP Basic 认证 |
| [canary.md](canary.md) | 全量切换、可远程切换的金丝雀流量插件 |
| [cors.md](cors.md) | 跨域资源共享（CORS） |
| [csrf.md](csrf.md) | CSRF 防护 |
| [ctx-setter.md](ctx-setter.md) | 上下文变量提取 |
| [direct-endpoint.md](direct-endpoint.md) | 直连指定 Endpoint |
| [dsl/README.md](dsl/README.md) | DSL 脚本引擎 |
| [dye.md](dye.md) | 流量标记请求头（条目级 `dye`——结果词表、value 模板、防伪造剥离） |
| [dynamic-upstream.md](dynamic-upstream.md) | 动态上游选择 |
| [forward-auth.md](forward-auth.md) | 外部认证委托 |
| [form-json-transform.md](form-json-transform.md) | 在扁平 Form URL 编码与 JSON 之间转换有界请求体 |
| [ext-proc.md](ext-proc.md) | 经 gRPC 的外部处理（兼容 Envoy ext_proc） |
| [global-access-control.md](global-access-control.md) | CDN 回源锁定：IP-CIDR + 密钥 header 复合放行规则，支持 profile 热切换 |
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
| [rate-limit.md](rate-limit.md) | 限流（内存，近似） |
| [rate-limit-redis.md](rate-limit-redis.md) | 限流（Redis，精确，集群级） |
| [real-ip.md](real-ip.md) | 获取客户端真实 IP |
| [region-route.md](region-route.md) | 区域路由 |
| [request-mirror.md](request-mirror.md) | 请求镜像 |
| [request-body-buffer.md](request-body-buffer.md) | 显式缓存请求 body，供后续阶段消费 |
| [request-restriction.md](request-restriction.md) | 请求过滤 |
| [response-rewrite.md](response-rewrite.md) | 响应重写 |
| [trace-context.md](trace-context.md) | W3C Trace Context 传播 |
| [wasm.md](wasm.md) | 通过 wasmtime 执行 proxy-wasm ABI guest 模块（WebAssembly） |
| [all-endpoint-status.md](all-endpoint-status.md) | 查询所有后端 Endpoint 状态 |
| [debug-access-log.md](debug-access-log.md) | 将 Access Log 注入响应头（仅开发用） |
