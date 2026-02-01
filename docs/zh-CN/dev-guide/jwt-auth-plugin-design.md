# JWT Auth 插件设计与配置（功能/配置准备）

本文档对应「插件开发」流程中的**功能、配置准备**阶段：定义插件结构体与配置项，对比 APISIX/Kong/Traefik/Envoy 的实现，确定功能逻辑与配置，供审核后再实现运行时逻辑与测试。

---

## 1. 目录与代码位置

- **插件目录**: `Edgion/src/core/plugins/edgion_plugins/jwt_auth/`
  - `mod.rs`: 导出 `JwtAuth`
  - `plugin.rs`: 插件实现（当前为 stub，审核通过后补全运行时逻辑）
- **配置类型**: `Edgion/src/types/resources/edgion_plugins/plugin_configs/jwt_auth.rs`
  - `JwtAuthConfig`、`JwtAlgorithm`
- **注册**: `EdgionPlugin::JwtAuth(JwtAuthConfig)` 已在 `edgion_plugin.rs` 与 `runtime.rs` 中注册。

---

## 2. 功能点对比：Edgion 设计 vs Kong vs APISIX

下表按功能维度对比 Edgion 当前设计与 Kong JWT、APISIX jwt-auth 的差异，便于评审与后续迭代。

| 功能点 | APISIX jwt-auth | Kong JWT | Edgion 设计 | 说明 |
|--------|-----------------|----------|-------------|------|
| **凭证模型** | Consumer + Credential（key/secret/public_key 等） | Consumer + JWT Credential（key/secret/rsa_public_key） | 路由级 `secret_ref`（单密钥）或 `secret_refs`（多密钥，K8s Secret） | Edgion 无独立 Consumer 资源，凭证来自 K8s Secret 引用 |
| **密钥存储** | etcd + 可选 APISIX Secret（env/ vault） | DB + 可选 vault 等 | 仅 K8s Secret（secret_ref / secret_refs） | Edgion 首版不做 env/vault 引用 |
| **Token 来源** | header / query / cookie（可配置名称） | Authorization / uri_param_names / cookie_names | header / query / cookie（可配置名称） | 三者一致 |
| **Token 优先级** | Header > Query > Cookie | 同左 | Header > Query > Cookie | 一致 |
| **算法** | HS256/HS512/RS256/ES256 | HS256/HS384/HS512/RS256/RS384/RS512/ES256/ES384/ES512 | HS256/HS384/HS512/RS256/ES256 | Edgion 首版少 RS384/RS512/ES384/ES512 |
| **多 key 选择** | payload 中 key_claim_name 对应值选 Consumer 凭证 | 同左（key_claim_name） | key_claim_name 选 secret_refs 中对应 Secret | 逻辑一致，数据源不同 |
| **hide_credentials** | 有（不把 token 传到上游） | 有 | 有 | 一致 |
| **匿名访问** | anonymous_consumer（消费者名） | anonymous（consumer id） | anonymous（消费者名） | 一致 |
| **key_claim_name** | 有（默认 key） | 有 | 有（默认 key） | 一致 |
| **exp/nbf 校验** | 有 | 有 | 有（+ claims_to_verify 可选） | 一致 |
| **时钟偏差** | lifetime_grace_period | - | lifetime_grace_period | Kong 文档未强调，Edgion 与 APISIX 一致 |
| **base64_secret** | 有（凭证级） | secret_is_base64（凭证级） | 首版未列，可后续加 | Edgion 可放在实现阶段 |
| **凭证级 exp** | 有（Consumer 凭证里可设 token 默认过期时间） | - | 无 | Edgion 不签发 token，仅校验 |
| **签发 Token** | 有（Admin API 为 Consumer 创建 JWT） | 无（仅验证） | 无 | Edgion 与 Kong 一致，只验证不签发 |
| **store_in_ctx** | 有（payload 存 ctx 供下游插件用） | - | 无 | Edgion 首版不做 |
| **转发到上游的头** | X-Consumer-Username、X-Credential-Identifier、自定义 Consumer 头 | 可配置 claim_to_headers 等 | X-Consumer-Username（与 BasicAuth 对齐） | Edgion 首版仅用户名/标识，可后续扩展 claim_to_headers |
| **JWKS / 远程公钥** | 无（凭证即密钥） | 无 | 无 | 三者首版均不做，Edgion 设计预留“后续可扩展 JWKS” |
| **iss/aud 校验** | 无 | 可选 | 无 | Edgion 首版不做 |
| **配置层级** | Consumer（凭证）+ Route/Service（行为） | Consumer（凭证）+ Route/Service（行为） | 仅 Route/Plugin 级（凭证通过 secret_ref(s) 引用） | Edgion 无独立 Consumer，配置更扁平 |

**差异小结**

- **Edgion 已有、与 APISIX/Kong 对齐**：Token 来源与优先级、多 key（key_claim_name）、hide_credentials、anonymous、exp/nbf、lifetime_grace_period、算法（首版子集）。
- **Edgion 与两者不同**：凭证来自 K8s Secret（secret_ref / secret_refs），无 Consumer 资源、无 Admin API 签发 Token。
- **Edgion 首版未做、可后续迭代**：base64_secret、store_in_ctx、更多算法（RS384/512、ES384/512）、claim_to_headers、JWKS/iss/aud。

---

## 3. 对标：APISIX / Kong（简述）

### 3.1 APISIX jwt-auth

- **Consumer/Credential**（凭证）: 每个 Consumer 可配置 jwt-auth 凭证：`key`（必填）、`secret`（HS*）、`public_key`（RS256/ES256）、`algorithm`、`exp`、`base64_secret`、`lifetime_grace_period`、`key_claim_name`。
- **Route/Service**（路由级）: `header`（默认 authorization）、`query`（默认 jwt）、`cookie`（默认 jwt）、`hide_credentials`、`key_claim_name`、`anonymous_consumer`、`store_in_ctx`。
- **Token 来源**: Header / Query / Cookie，优先级一般为 Header > Query > Cookie。
- **验证通过后**: 设置 `X-Consumer-Username`、`X-Credential-Identifier` 等请求头转发到上游。

### 3.2 Kong JWT

- 与 APISIX 类似：JWT 插件验证签名与声明，支持从 Header/Query/Cookie 取 token，可配置匿名消费者、隐藏凭证等。

---

## 4. 对标：Traefik / Envoy / Tyk（差异与取舍）

| 能力 | Traefik | Envoy | Edgion 设计 |
|------|---------|--------|-------------|
| 密钥来源 | signingSecret / publicKey / jwksFile / jwksUrl | local_jwks / remote_jwks（含 issuer） | 首版：secret_ref / secret_refs（K8s Secret），后续可扩展 JWKS URL |
| 多 Issuer | trustedIssuers + JWKS | 多 Provider + rules | 首版：多凭证通过 secret_refs + key_claim_name 区分 |
| Token 位置 | Header / Form / Query | from_headers / from_params / from_cookies | header / query / cookie（与 APISIX 一致） |
| 声明校验 | claims（自定义规则） | exp、aud、iss 等 | exp、nbf（可选 claims_to_verify）+ lifetime_grace_period |
| 转发声明 | forwardHeaders | claim_to_headers | 首版：可设 X-Consumer-Username 等，与 BasicAuth 对齐 |
| 匿名访问 | - | - | anonymous（与 BasicAuth 一致） |

**Edgion 首版取舍**:

- **不做**：JWKS URL、iss/aud 校验、自定义 claim 规则（可后续迭代）。
- **做**：单密钥（secret_ref）或多密钥（secret_refs + key_claim_name）、HS256/384/512 与 RS256/384/512、ES256/384/512、Header/Query/Cookie 取 token、hide_credentials、anonymous、exp/nbf 与时钟偏差。
- **无 Consumer**：Edgion 无独立 Consumer 资源，凭证全部来自 K8s Secret（secret_ref / secret_refs），配置仅路由/插件级。

---

## 5. 功能目标（确定逻辑）

1. **Token 提取**（优先级：Header > Query > Cookie）  
   - Header：默认 `authorization`，支持 `Bearer <token>` 或裸 token。  
   - Query：默认参数名 `jwt`。  
   - Cookie：默认名称 `jwt`。

2. **凭证模型**  
   - **单 issuer**：`secret_ref` 指向一个 Secret，包含 `secret`（HS*）或 `publicKey`（RS*/ES*），不按 payload 中的 key 查找。  
   - **多 key**：`secret_refs` 指向多个 Secret，每个 Secret 含 `key` + `secret` 或 `key` + `publicKey`；使用 JWT payload 中 `key_claim_name` 对应字段的值选择凭证。

3. **算法**  
   - 对称：HS256、HS384、HS512。  
   - 非对称 RSA：RS256、RS384、RS512。  
   - 非对称 ECDSA：ES256、ES384。  
   - 注：ES512（P-521）因底层库限制暂不支持。

4. **声明与时间**  
   - 校验 `exp`、`nbf`（若存在）。  
   - `lifetime_grace_period`（秒）用于时钟偏差。  
   - 可选 `claims_to_verify` 显式指定要校验的声明（如 `["exp","nbf"]`）。

5. **匿名与上游**  
   - 未携带或验证失败时：若配置 `anonymous`，则放行并设置匿名消费者标识（如 X-Consumer-Username）；否则返回 401。  
   - 验证通过：设置 X-Consumer-Username（可为 payload 中某 claim，如 `key` 或后续扩展）、可选隐藏凭证（`hide_credentials`）。

6. **错误响应**  
   - 无 token / 无效 token / 签名或声明校验失败：401 Unauthorized，可返回 JSON 或纯文本（与 BasicAuth 风格一致）。

---

## 6. 配置项（已落地的 JwtAuthConfig）

| 字段 | 类型 | 默认 | 说明 |
|------|------|------|------|
| secret_ref | SecretObjectReference? | - | 单密钥：Secret 含 `secret` 或 `publicKey` |
| secret_refs | []SecretObjectReference? | - | 多密钥：每个 Secret 含 `key` + `secret` 或 `key` + `publicKey` |
| algorithm | JwtAlgorithm | HS256 | HS256/HS384/HS512/RS256/ES256 |
| header | string | "authorization" | 取 token 的 Header 名 |
| query | string | "jwt" | Query 参数名 |
| cookie | string | "jwt" | Cookie 名 |
| hide_credentials | bool | false | 是否不把 token 转发到上游 |
| anonymous | string? | - | 匿名消费者名，允许未认证访问 |
| key_claim_name | string | "key" | Payload 中用于选择凭证的 claim 名 |
| lifetime_grace_period | u64 | 0 | exp/nbf 时钟偏差（秒） |
| claims_to_verify | []string? | - | 可选，如 ["exp","nbf"] |

**约束**（可在 handler 或运行时校验）：  
- 至少配置 `secret_ref` 或 `secret_refs` 之一。  
- 使用 `secret_refs` 时，必须配置 `key_claim_name` 与算法一致（HS* 用 secret，RS*/ES* 用 publicKey）。

---

## 7. Secret 数据格式（约定）

- **secret_ref（单密钥）**  
  - HS*: Secret 含 key `secret`（原始或 base64 由后续实现决定）。  
  - RS*/ES*: Secret 含 key `publicKey`（PEM）。

- **secret_refs（多密钥）**  
  - 每个 Secret：  
    - HS*: `key`（标识符）+ `secret`。  
    - RS*/ES*: `key`（标识符）+ `publicKey`（PEM）。

---

## 8. 与现有插件的一致性

- 与 **BasicAuth** 对齐：`hide_credentials`、`anonymous`、401 与上游头（X-Consumer-Username）。  
- 与 **EdgionPlugins** 一致：通过 `RequestFilterEntry` 与 `PluginRuntime::add_from_request_filters` 注册，支持条件执行（conditions）。

---

## 9. 审核后待做（不在此文档实现）

- 实现插件**运行时逻辑**（解析 JWT、选密钥、校验签名与 exp/nbf、设置头/匿名/401）。  
- **凭证加载**：从 K8s Secret 解析 secret_ref / secret_refs（若需在 conf sync 或请求路径拉取，需定方案）。  
- **单元测试**：mock session，覆盖有 token/无 token/错误 token/匿名/hide_credentials。  
- **集成测试**：脚本与配置目录按「插件开发」文档（`run_integration_test.sh`、`EdgionPlugins/PluginJwtAuth`、client suite）。  
- **用户文档**：`Edgion/docs/zh-CN/user-guide/edgion-plugins/jwt-auth.md`（或对应 path）。

---

**当前状态**：目录已创建，配置与 stub 已接入，等待对**功能逻辑**与**配置项**的审核；审核通过后再进行「审核后开始编码」与集成/用户文档阶段。
