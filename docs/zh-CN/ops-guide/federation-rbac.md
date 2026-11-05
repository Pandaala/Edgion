# 联邦 RBAC

Controller 通过评估从实际匹配路由派生出的 `(verb, kind)` 对，对联邦 Center 在 Admin API 上执行的每个操作进行授权。本文档说明授权模型、内置默认策略、启动时生效的开关及常见配置示例。

## 工作原理

每个 Center 请求（盲代理 UI 调用、同步操作、failover）都经过与本地管理请求相同的授权层。Center 被分配受限标识（`CenterRestricted`），其允许的操作由 `[center].rbac` 配置块管理。本地管理员 Token 映射到 allow-all（通配符）策略，Center 授权不影响它。

授权按 `(verb, kind)` 决定：

- **verb**——操作类型：`get`、`list`、`list-keys`、`watch`、`create`、`update`、`delete`、`failover`、`server-info`、`diagnostics` 等。
- **kind**——PascalCase 资源种类（如 `EdgionConfigData`、`HTTPRoute`、`RegionRoute`）。Kind 匹配**区分大小写**；使用规范 PascalCase 拼写。Verb 不区分大小写。

授权**不是基于对象**的：`EdgionConfigData` 写入授权覆盖所有 EdgionConfigData 对象，而非特定命名对象。

## 内置默认值（省略 `rbac` 时）

当 `[center]` 已配置但 `rbac` 字段不存在时，会自动应用安全的内置默认策略：

| Verb | Kind | 是否允许 |
|------|------|---------|
| `get`、`list`、`watch`、`list-keys` | **除 `Secret` 外**的所有资源 | 是 |
| `get`、`list`、`watch`、`list-keys` | `Secret` | **否**——凭据安全 |
| `create`、`update`、`delete` | `EdgionConfigData` | 是 |
| `create`、`update`、`delete` | 任何其他 kind | 否 |
| `list`、`failover` | `RegionRoute`（合成标签，非资源 kind） | 是 |
| `server-info` | `NonResource`（合成标签，非资源 kind） | 是 |
| `diagnostics` 及其他 | 任何 | 否 |

关键说明：

- `EdgionTls` 在默认策略下**可读**（TLS 配置，非通过 Secret 暴露的原始密钥材料），只有 `Secret` kind 被排除。
- `EdgionConfigData` 写入授权覆盖**所有**插件元数据，而非仅限 region-route 或 IP-restriction 记录。这是 kind 级粒度的已知限制。
- `RegionRoute` 是合成分类标签（非 `ResourceKind` 变体）。`list` 覆盖 effective 读取端点（`GET /api/v1/region-routes/effective`）；`failover` 覆盖 POST failover 端点。`get` **故意不授予**：没有任何分类器路由产生 `(get, RegionRoute)`，授予它只会是一条死权限。这两个 verb 与 ResourceKind 读取规则分开，因为 `RegionRoute` 不参与穷举性验证。

存在显式 `rbac` 块时，它**完全覆盖**内置默认。不进行合并。

## `center.enabled` 开关

```yaml
center:
  enabled: false   # 默认：true
```

设置 `enabled: false` 后重启 Controller，即可禁用其联邦客户端。`[center]` 下的所有字段（包括 `rbac` 和 `address`）都在启动时读取。修改后需重启所有使用该配置的 Controller 副本。资源更新和证书轮换仍通过各自的 watch 路径动态生效。

## 授权示例

### 1. 默认（省略 `rbac`）

覆盖 Center 的自动化操作（IP-restriction 同步、region-route 同步、failover）以及对除 `Secret` 外所有资源 kind 的读取访问。适合大多数标准部署。

```yaml
center:
  address: "https://center.example.com:12251"
  name: "ctrl-1"
  cluster: "prod"
  security:
    active: primary
    certs:
      - name: primary
        cert: certs/grpc/primary/client.crt
        key: certs/grpc/primary/client.key
        ca: certs/grpc/primary/ca.crt
  # rbac 省略——使用内置默认策略
```

### 2. 严格只读

适用场景：Center 仅允许观察资源（用于仪表板/审计目的），无写入权限，无 failover。

```yaml
center:
  address: "https://center.example.com:12251"
  name: "ctrl-1"
  cluster: "prod"
  security:
    active: primary
    certs:
      - name: primary
        cert: certs/grpc/primary/client.crt
        key: certs/grpc/primary/client.key
        ca: certs/grpc/primary/ca.crt
  rbac:
    allow:
      - verbs: [get, list, watch]
        kinds: [HTTPRoute, GRPCRoute, TCPRoute, Service, EdgionConfigData,
                EdgionGatewayConfig, EdgionTls]
      - verbs: [get, list]
        kinds: [RegionRoute]
```

注意：明确枚举具体 kind。**不要**对读取 verb 使用 `kinds: ["*"]`——见[安全说明](#安全说明)。

### 3. 仅管理 region-route 和 IP-restriction

适用场景：Center 管理 region-routing 和 IP-restriction 策略（含 region-route failover），但不应读取其他资源。

```yaml
center:
  address: "https://center.example.com:12251"
  name: "ctrl-1"
  cluster: "prod"
  security:
    active: primary
    certs:
      - name: primary
        cert: certs/grpc/primary/client.crt
        key: certs/grpc/primary/client.key
        ca: certs/grpc/primary/ca.crt
  rbac:
    allow:
      - verbs: [get, list, watch, create, update, delete]
        kinds: [EdgionConfigData]
      - verbs: [failover]
        kinds: [RegionRoute]
```

### 4. 完全控制——附安全警告

适用场景：**仅限受信任的内部部署，Center 运维和 Controller 运维是同一团队，且您已接受以下风险。**

> **警告——启用前请阅读[安全说明](#安全说明)。**
> `kinds: ["*"]` 与读取 verb 结合时会暴露 `Secret` 和 `EdgionTls` 的私钥材料。`verbs: ["*"]` 允许 Center 修改任何资源。`"*":"*"` 等价于授予 Center 对 Controller 的完全控制权。

```yaml
center:
  address: "https://center.example.com:12251"
  name: "ctrl-1"
  cluster: "prod"
  security:
    active: primary
    certs:
      - name: primary
        cert: certs/grpc/primary/client.crt
        key: certs/grpc/primary/client.key
        ca: certs/grpc/primary/ca.crt
  rbac:
    allow:
      - verbs: ["*"]
        kinds: ["*"]
```

### 5. 自定义最小权限

适用场景：已启用特定 Center 功能，想精确授予所需权限——不多不少。

```yaml
center:
  address: "https://center.example.com:12251"
  name: "ctrl-1"
  cluster: "prod"
  security:
    active: primary
    certs:
      - name: primary
        cert: certs/grpc/primary/client.crt
        key: certs/grpc/primary/client.key
        ca: certs/grpc/primary/ca.crt
  rbac:
    allow:
      # Center 同步：读写插件元数据
      - verbs: [get, list, watch, create, update, delete]
        kinds: [EdgionConfigData]
      # 区域 failover
      - verbs: [failover]
        kinds: [RegionRoute]
      # 仪表板读取：仅枚举特定 kind
      - verbs: [get, list]
        kinds: [HTTPRoute, GRPCRoute, Service, EdgionGatewayConfig]
```

## 安全说明

### 通配符读取会暴露凭据材料

联邦路径上**没有服务端 Secret 兜底**。当 `kinds: ["*"]` 与 `get`、`list` 或 `watch` 结合时，Center 可以通过盲代理读取 `Secret` 和 `EdgionTls` 资源的内容——包括私钥和证书。

内置默认值通过排除 `Secret` kind 来避免这一问题。任何显式 `rbac` 块均由运维人员全权负责。

**对读取授权始终枚举具体 kind。** 使用示例 2 或 5 作为模板，而非 `kinds: ["*"]`。

### 宽泛的写入授权

- 写入授权（`create`、`update`、`delete`）允许 Center 修改所授权 kind 的资源。`EdgionConfigData` 写入授权覆盖**所有**插件元数据对象，而非特定对象。
- `server-info` 和 `diagnostics` 会暴露进程和配置详情。

### 无对象级粒度

`(verb, kind)` 模型限定了允许对哪些资源类型执行哪些操作。它**不**限制同一 kind 中哪些命名对象可以被访问。`update:EdgionConfigData` 授权允许更新任何 EdgionConfigData 对象，而非仅特定对象。

### 规范 kind 大小写

`rbac` 块中的 Kind 名称与规范 PascalCase Kind 名称**区分大小写**匹配。小写或别名拼写（如 `edgionconfigdata`、`httpRoute`）永远静默不匹配。始终使用规范形式：`EdgionConfigData`、`HTTPRoute`、`GRPCRoute`、`RegionRoute`、`Service`、`EdgionTls` 等。

---

## CLI Token RBAC

CLI Token 允许 `edgion-cli`（或任何 HTTP 客户端）使用携带细粒度 RBAC 权限的**静态 Bearer Token** 调用 Controller Admin API，无需交互式登录。这是 CI 管道、自动化脚本和必须非交互式调用 Admin API 的服务账号的推荐方式。

授权模型与[Center RBAC](#工作原理)相同：每个 Token 携带 `[center].rbac` 风格的 `FedRbacConfig`（相同的 `verbs × kinds` 分类法，相同的默认拒绝行为，相同的规范大小写规则）。Verb 和 kind 的完整列表及大小写规则见上方[工作原理](#工作原理)。

### 生成 Token

```bash
# 1. 生成新 Token（32 字节 CSPRNG，URL-safe base64，edg_ 前缀）
edgion-cli gen-token
```

输出：

```
Token (use with --token or EDGION_TOKEN):
  edg_<base64url>

SHA-256 (paste into controller cli_tokens[].token_sha256):
  <64 lowercase hex chars>
```

SHA-256 是对**完整的 `edg_` 前缀 Token** 计算的——即作为 `Authorization: Bearer <token>` 发送的完整字符串。这是要粘贴到配置中的值。

### Controller 配置

在 Controller YAML 配置中添加 `cli_tokens` 块（可与 `center` 和 `admin_token` 共存）：

```yaml
cli_tokens:
  # 只读——适合监控管道
  - name: monitor-bot
    token_sha256: "<gen-token 生成的 SHA-256>"
    rbac:
      allow:
        # 枚举具体 kind；对读取 verb 切勿使用 kinds: ["*"]。
        - verbs: [get, list]
          kinds: [HTTPRoute, GRPCRoute, TCPRoute, Service,
                  EdgionGatewayConfig, EdgionTls, EdgionConfigData]

  # 范围有限的读写——管理插件元数据
  - name: plugin-manager
    token_sha256: "<gen-token 生成的 SHA-256>"
    rbac:
      allow:
        - verbs: [get, list, watch, create, update, delete]
          kinds: [EdgionConfigData]
```

修改配置后重启 Controller。标准 Kubernetes 部署可执行：

```bash
kubectl -n edgion-system rollout restart deployment/edgion-controller
kubectl -n edgion-system rollout status deployment/edgion-controller
```

### 使用 CLI Token

```bash
# 通过标志（仅长选项；-t 已被 --target 占用）
edgion-cli --token edg_<base64url> get httproute

# 通过环境变量（CI 推荐方式——避免 Token 出现在 shell 历史记录中）
export EDGION_TOKEN=edg_<base64url>
edgion-cli get httproute -n production
```

优先级：`--token` > `EDGION_TOKEN` > 缓存的登录 Token（`~/.edgion/token`，由 `edgion-cli login --token` 写入）。

验证是惰性的：第一个真实请求返回 `401 Unauthorized`（未知 Token）或 `403 Forbidden`（权限不足）。唯一的公开认证端点是 `GET /api/v1/auth/status`。

### 安全说明

**1. 宽泛的 verb 授权等于完整管理员权限。**
`rbac` 授予 `verbs: ["*"]` 覆盖完整 admin router（包括 `failover`、`acme-trigger`、`conf-sync-rotate`、`server-info` 和 `diagnostics`）的 Token 实际上等同于 allow-all 管理员 Token。授予消费方实际需要的最小 verb 集。

**2. 通配符读取会暴露凭据材料。**
CLI Token 路径上没有服务端 `Secret` 兜底。对读取 verb 使用 `kinds: ["*"]` 允许 Token 通过盲代理读取 `Secret` 和 `EdgionTls` 的私钥材料。对所有读取授权始终枚举具体 kind。见上方[通配符读取会暴露凭据材料](#通配符读取会暴露凭据材料)。

**3. Verb 和 Kind 字符串使用规范大小写。**
Verb 字符串不区分大小写匹配（`get`、`GET`、`Get` 均有效）。Kind 字符串与规范 PascalCase Kind 名称**区分大小写**匹配。大小写错误的 kind（如 `edgionconfigdata`）永远静默不匹配任何请求。始终使用规范 PascalCase：`EdgionConfigData`、`HTTPRoute`、`Service` 等。

**4. Token 轮换。**
在配置中替换 `token_sha256`，然后重启每个 Controller 副本。每个实例在退出前仍使用启动时读取的 Token 表；只有所有实例都以新配置重启后，撤销才全部生效。新 `edg_` Token 的分发应与此次滚动更新协调。

**5. `--token` 和 `EDGION_TOKEN` 适用于 CI/自动化。**
它们**不**修改 `~/.edgion/token`（无缓存——每次调用是临时的）。缓存的登录 Token 不受影响。建议使用 `EDGION_TOKEN` 而非 `--token`，以避免 Token 泄露到 shell 历史记录或进程列表中。

**6. 迁移：无认证模式已移除。**
以前绕过认证的部署，现在必须配置 `admin_token`（或 `EDGION_ADMIN_TOKEN`）和/或 `cli_tokens`。两者均未设置时，Admin API 以 503 失败关闭。纯 Token 认证模型及升级路径见 [Controller 配置](configuration/controller-config.md#认证)。

**7. `list-keys` 会暴露资源名称。**
`list-keys` verb 即使未授予完整的 `get` 或 `list` 访问权限，也会暴露资源名称（以及 namespace/kind 元数据）。如果资源名称的存在是敏感信息，将 `list-keys` 视为读取授权并明确枚举 kind，而非使用 `kinds: ["*"]`。

**8. `EDGION_TOKEN` / `--token` 不使用缓存的会话 Token。**
这些路径完全绕过 `ensure_auth`，不读取也不写入 `~/.edgion/token`。每次调用对本地 Token 缓存完全无状态。

**9. 集成测试豁免仅限测试。**
`wipe-all` 和 `debug` verb，以及集成测试模式标志，是完全绕过正常 RBAC 检查的仅测试入口。**绝对不要在生产中启用它们。** CLI Token 策略不管辖这些路径——它们由 Controller 进程的 `--integration-testing-mode` 启动标志保护，而非 RBAC 引擎。

### 配置 schema 参考

完整字段参考和两 Token 示例，见 [Controller 配置](configuration/controller-config.md#cli_tokens----静态-cli-token)。

---

## 相关文档

- [Controller 配置](configuration/controller-config.md)
- [edgion-cli](edgion-cli.md)
