# JWT Auth 插件设计

本文档说明 JwtAuth 当前已经实现的配置、Controller 解析流程和 Gateway 请求行为。JwtAuth 使用 Kubernetes Secret 支持的 issuer 组验证 JWT；它不签发 Token、不获取 JWKS，也不执行 OIDC discovery。

---

## 1. 代码位置

- Gateway 运行时：`edgion-gateway/src/plugins/http/jwt_auth/`
- 资源 Schema：`edgion-resources/src/resources/edgion_plugins/plugin_configs/jwt_auth.rs`
- Controller 凭证解析器：`edgion-controller/src/conf_mgr/sync_runtime/resource_processor/handlers/plugins/resolve_credential/jwt.rs`
- 插件注册：`EdgionPlugin::JwtAuth(JwtAuthConfig)`

---

## 2. 配置模型

`issuerGroups` 是面向操作员的凭证模型。每个组包含：

- 唯一的组名。
- 一个或多个精确的 `iss` 值。
- 一个或多个 Kubernetes `secretRefs`。
- 可选的组级 `algorithm` 和 `audiences` 覆盖值。

插件级 `algorithm` 和 `audiences` 提供默认值。Controller 将这些默认值快照合并到每个解析后的组。组级 `audiences: None` 继承插件级值；组级 `audiences: []` 会显式覆盖非空的插件级默认值并关闭校验。只有生效值为 `None`（两级都未设置）或空列表时才关闭校验。

每个引用的 Secret 表示一个凭证：

- `key`：由 `keyClaimName` 指定的 JWT payload 声明选择的标识符，默认声明名为 `key`。
- `secret`：HS256、HS384 或 HS512 使用的 HMAC 密钥材料。
- `publicKey`：RS256、RS384、RS512、ES256 或 ES384 使用的 PEM 公钥。

JOSE header 中的 `kid` 或 `keyid` 不参与凭证选择。选择器只从配置的 payload 声明读取。

---

## 3. 配置字段

| 字段 | 类型 | 默认值 | 运行时含义 |
|------|------|--------|------------|
| `issuerGroups` | Array | 无 | 命名的 issuer 和 Secret 组；未配置 `anonymous` 时必需 |
| `algorithm` | `JwtAlgorithm` | `HS256` | 插件级默认值，可被组级值覆盖 |
| `audiences` | Array | 无 | 插件级默认 audience allow-list，可被组级值覆盖 |
| `header` | String | `authorization` | 第一 Token 来源；支持 `Bearer <token>` 或裸 Token |
| `query` | String | `jwt` | Query 参数名 |
| `allowTokenInQuery` | Boolean | `false` | 启用 Query 来源；默认禁用以避免 URL 暴露凭证 |
| `cookie` | String | `jwt` | Cookie Token 来源 |
| `hideCredentials` | Boolean | `false` | 代理前移除本次选中的 Token 来源 |
| `anonymous` | String | 无 | 仅允许未携带 Token 的请求，并写入匿名请求头 |
| `keyClaimName` | String | `key` | 在路由到的组内选择凭证所用的 payload 声明 |
| `lifetimeGracePeriod` | Integer | `0` | `exp` 和已存在 `nbf` 声明的宽限秒数 |
| `base64Secret` | Boolean | `false` | `false` 直接使用 Controller 提供的 UTF-8 字节；`true` 在 Kubernetes 解码 Secret `.data` 外层后，再把该字符串按 standard Base64 解码 |
| `maximumExpiration` | Integer | `0` | `exp` 距当前时间的最大值；`0` 关闭该附加上限 |
| `authFailureDelayMs` | Integer | `0` | 缺少 Token 和 Token 无效时，在返回 401 前等待的毫秒数 |
| `realm` | String | `jwt` | Bearer `WWW-Authenticate` challenge 使用的 realm |
| `storeClaimsInCtx` | Boolean | `false` | 将已验证声明存入 `jwt_claims`，供下游插件使用 |
| `claimsToHeaders` | Object | 无 | 将已验证声明显式映射到受大小限制的上游请求头 |

Kubernetes 总会先移除 Secret `.data` 的外层 Base64 编码，之后 Controller 才读取其值。`base64Secret: false` 直接使用得到的 UTF-8 `secret` 字节。`base64Secret: true` 则要求该 UTF-8 字符串本身是 standard Base64，并再次解码为实际的 HMAC key 字节。

---

## 4. Controller 解析

对于每一代配置，Controller 会：

1. 清除上一轮解析产生的传输状态。
2. 通过标准 Secret 和 ReferenceGrant 路径解析每个 `secretRefs` 条目。
3. 从每个可用 Secret 读取 `key`，以及 `secret` 或 `publicKey`。
4. 组内出现重复 `key` 时保留第一个凭证。
5. 快照合并组级或插件级算法与 audience 策略。
6. 按每个配置的精确 issuer 字符串索引解析后的组。

解析后的 map 由 Controller 管理，在配置输出中经过脱敏，不进入 CRD Schema，并传输到 Gateway。Secret 变化会重新排队其所属插件资源并发布新一代插件。

Gateway 构造 JwtAuth 时会消费该解析 map，一次性准备不可变的解码密钥和验证策略，并从保留的配置中移除原始解析 map。请求处理只在当前 generation 中执行只读查找。

---

## 5. 请求流程

JwtAuth 按以下优先级提取一个 Token：

1. Header。
2. Query，仅当 `allowTokenInQuery` 为 `true`。
3. Cookie。

未携带 Token 时，`anonymous` 可以放行请求，并写入 `X-Anonymous-Consumer: true` 和 `X-Consumer-Username: <anonymous>`。未配置 `anonymous` 时，插件应用 `authFailureDelayMs` 后返回固定 401 响应。

请求携带 Token 时，JwtAuth 必须验证该 Token。Token 格式错误、过期、尚未生效、签名错误、作用域错误或其他无效情况，即使配置了 `anonymous` 也会返回固定 401。

验证顺序是有意固定的：

1. 解析 JOSE header。
2. 解码不可信的 payload 选择器，但不把它们作为已认证输出。
3. 要求非空 `iss`，并精确路由到对应的 issuer 组。
4. 要求 `header.alg` 与解析后的组算法一致。
5. 读取配置的 payload key 声明，并且只在路由到的组内查找该 key。
6. 使用准备好的组策略验证签名和注册声明。
7. 配置后应用 `maximumExpiration`。

未签名的选择器值不会进入成功结果。只有完成全部验证后，声明才可使用。

---

## 6. 声明校验

- `exp` 必须存在并接受校验。
- `nbf` 可选；存在时接受校验。
- `lifetimeGracePeriod` 同时用于 `exp` 和 `nbf` 校验。
- 生效的 `audiences` 非空时，必须存在 `aud`，且至少有一个 audience 匹配。
- 组级 `None` 继承插件级值；组级 `[]` 会覆盖非空的插件级默认值。只有合并后的生效值为 `None`（两级都未设置）或空列表时才关闭校验。
- `maximumExpiration` 接受 JWT 库相同的 NumericDate 整数和浮点形式，并在应用上限前使用相同的舍入语义。

精确 `iss` 路由、解析后的算法和组内 key 查找可防止 issuer/key 交叉和算法混淆。

---

## 7. 成功与失败行为

验证成功后，JwtAuth 会：

- 在 PluginLog 中记录 `OK group=<name> iss=<iss> u=<sub>`；`sub` 按敏感数据处理，并由配置的策略进行掩码。`sub` 缺失或不是字符串时使用 `(missing sub)`。
- 只写入 `claimsToHeaders` 显式配置的请求头。
- 可选地将已验证声明存入 `jwt_claims`。
- 可选地移除本次请求选择的凭证来源。

JwtAuth 默认不会根据 `sub` 写入 `X-Consumer-Username`。

所有认证失败使用同一种公开响应形态：

- 状态码：401。
- Content-Type：`text/plain`。
- 响应体：`401 Unauthorized - Invalid or missing JWT`。
- 缺少 Token：`WWW-Authenticate: Bearer realm="<realm>"`。
- Token 存在但无效：使用相同 realm，并添加 `error="invalid_token"` 和通用 `error_description="Unauthorized - Invalid or missing JWT"`。

失败 PluginLog 条目是静态分类，不包含 Token、issuer、group、算法、密钥标识、密钥材料或声明。

---

## 8. 边界

- JwtAuth 使用 Controller 解析后的 Kubernetes Secret；请求路径不获取 Secret。
- JwtAuth 不获取 JWKS、不执行 OIDC discovery、不签发 Token，也没有 Consumer 资源。
- 需要 OIDC/JWKS provider 行为时，请使用 OpenID Connect 插件。
- Query Token 保持显式开启，因为 URL 可能通过日志、浏览器历史和 referrer 暴露 bearer 凭证。
- Header 投影是显式且有界的；隐式添加身份请求头不属于插件的当前契约。
