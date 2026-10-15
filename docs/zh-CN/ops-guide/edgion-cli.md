# edgion-cli 命令行工具

`edgion-cli` 是 Edgion 的命令行管理工具，用于查看和管理网关资源配置。

## 安装

编译后的二进制文件位于 `target/release/edgion-cli` 或 `target/debug/edgion-cli`。

## 基本用法

```bash
edgion-cli [OPTIONS] <COMMAND>
```

### 全局选项

| 选项 | 简写 | 说明 | 默认值 |
|------|------|------|--------|
| `--target` | `-t` | 目标 API 类型 | `center` |
| `--server` | - | 服务器地址 | 根据 target 自动选择 |
| `--socket` | - | Unix socket 路径 | - |
| `--token` | - | 静态 CLI Token（长选项，`-t` 已被 `--target` 占用） | - |
| `--help` | `-h` | 显示帮助信息 | - |
| `--version` | `-V` | 显示版本信息 | - |

`edgion-cli` 采用**纯 Token**方式：使用单个 Bearer Token（Istio 风格）向 Controller Admin API 认证。CLI 中不再有用户名/密码登录，也没有 OIDC。Token 可以是本地管理员 Token，也可以是以 `edg_` 为前缀的 `cli_tokens` Token。Token 优先级（从高到低）：

1. `--token <TOKEN>` 标志（仅长选项，`-t` 已被 `--target` 占用）
2. `EDGION_TOKEN` 环境变量（避免 Token 暴露在 shell 历史记录中）
3. 位于 `~/.edgion/token` 的缓存登录 Token，由 `edgion-cli login --token` 写入

验证为惰性：第一个请求返回 `401`（未知 Token）或 `403`（权限不足）。唯一的公开认证端点是 `GET /api/v1/auth/status`。

## Target 类型

`edgion-cli` 支持三种 target 类型，用于连接不同的数据源：

| Target | 组件 | 默认端口 | 支持的命令 | 说明 |
|--------|------|----------|------------|------|
| `center` | Controller/ConfCenter | 12101 | get, apply, delete, reload | 完整 CRUD 操作 |
| `server` | Controller/ConfigServer | 12101 | get (只读) | 查看 ConfigServer 缓存 |
| `client` | Gateway/ConfigClient | 12001 | get (只读) | 查看 Gateway 缓存 |

### 使用示例

```bash
# center (默认) - 完整功能
edgion-cli get httproute
edgion-cli apply -f route.yaml
edgion-cli delete httproute my-route -n default

# server - 查看 Controller 的 ConfigServer 缓存
edgion-cli -t server get httproute
edgion-cli -t server get httproute -n prod

# client - 查看 Gateway 的 ConfigClient 缓存
edgion-cli -t client get httproute
edgion-cli -t client --server http://gateway:12001 get service -n default
```

## 命令详解

### wasm - 本地检查与 Envoy 迁移

`wasm` 命令组完全在本地运行：不会解析 CLI Token、创建 Admin API
客户端、执行认证或发起网络请求。

```bash
edgion-cli wasm inspect --file ./plugin.wasm --format text
edgion-cli wasm inspect --file ./plugin.wasm --format json

edgion-cli wasm convert-envoy \
  --file ./envoy-wasm.yaml \
  --format yaml \
  --report ./conversion-report.json
```

`inspect` 支持 `text`、`json`、`yaml`，最多读取 64 MiB。模块畸形或不兼容时，
它仍会输出兼容性报告，但命令返回非零。

报告版本 2 标明 `edgion-proxy-wasm-http-v1` 和 `static_load_compatibility`。
`compatible: true` 不代表执行或实例化验证。请检查 import/hook 支持状态和警告：
`accepted_noop` 返回 OK 但不执行语义，`known_unsupported_trap` 在调用时 trap。
文本输出会转义模块名称和诊断中的控制字符，避免执行终端控制序列或插入报告行；
JSON/YAML 结构化报告保留原值。

`convert-envoy` 只接受一个 `envoy.filters.http.wasm` wrapper、其 v3
`typed_config`，或一个裸 `envoy.extensions.wasm.v3.PluginConfig`。它不会遍历
bootstrap、listener 或 filter chain。stdout 是可放入 EdgionPlugins 条目
`config` 字段的 `WasmConfig` 片段；转换器不会猜测 resource namespace 或
plugin stage，用户必须显式选择 Edgion stage。

转换严格执行，且没有有损放行选项。不支持的 runtime、context identity、非空
Envoy plugin name、环境变量或 capability 注入、基于 cluster 的远程拉取、
reload/cache 行为、冲突 oneof、未知字段和任意 `google.protobuf.Any` 都会成为 blocker。出现 blocker 时 stdout
不生成配置；可选 JSON 报告仍会以拒绝覆盖的方式创建；命令返回非零。
未指定 `--report` 时，同一份结构化报告写入 stderr，stdout 仍专用于生成的配置。

Envoy 会把非空 `PluginConfig.name` 作为 `plugin_name` 暴露给 guest。Edgion 的
`plugin_name` 属性固定为 `Wasm`，Edgion resource name 不能替代该值，因此转换器
只接受空的 Envoy plugin name。空 name 仍可转换，但报告会警告：Envoy 向 guest
暴露的是空 `plugin_name`；应测试 guest 不依赖该属性。

Envoy runtime 为空时允许转换，但报告会警告，并明确固定为 Edgion 的 Wasmtime
runtime。合法的 Envoy v3 远程 `HttpUri` 必须包含非空 cluster，而其传输与安全语义
无法恢复，因此远程来源始终阻止自动输出。报告可以给出 URL、timeout、retry 次数
和基础退避的暂定映射，供手工迁移参考；它不是兼容的生成配置。

这是迁移助手，不是“Envoy 无缝兼容”的证明。Edgion 使用其文档定义的 Wasmtime
Host ABI；fail-close 行为存在差异；guest callout 默认关闭；上游响应 body 始终
流式传输，Wasm 只能逐 chunk 观察。转换器不会下载远程模块、读取 Envoy
`source.file`、解析 cluster 或 Secret、读取环境变量，也不会改变响应缓冲策略。

### get - 获取资源

获取单个资源或列出资源列表。

```bash
edgion-cli get <KIND> [NAME] [OPTIONS]
```

**参数:**
- `KIND`: 资源类型（如 httproute, service, gateway）
- `NAME`: 资源名称（可选，不指定则列出所有）

**选项:**
- `-n, --namespace <NS>`: 指定命名空间
- `-o, --output <FORMAT>`: 输出格式（table, json, yaml, wide）

**示例:**

```bash
# 列出所有 HTTPRoute
edgion-cli get httproute

# 列出指定命名空间的 HTTPRoute
edgion-cli get httproute -n production

# 获取指定资源，输出 YAML 格式
edgion-cli get httproute my-route -n default -o yaml

# 获取指定资源，输出 JSON 格式
edgion-cli get service backend-svc -n default -o json
```

**支持的资源类型:**

| 类型 | 说明 |
|------|------|
| httproute | HTTP 路由规则 |
| grpcroute | gRPC 路由规则 |
| tcproute | TCP 路由规则 |
| udproute | UDP 路由规则 |
| tlsroute | TLS 路由规则 |
| service | Kubernetes Service |
| endpointslice | EndpointSlice |
| endpoint | Endpoints |
| gateway | Gateway 资源 |
| gatewayclass | GatewayClass 资源 |
| edgiontls | Edgion TLS 配置 |
| edgionplugins | Edgion HTTP 插件配置 |
| edgionstreamplugins | Edgion Stream 插件配置 |
| edgionconfigdata | 插件元数据 |
| linksys | LinkSys 配置 |
| referencegrant | ReferenceGrant |
| backendtlspolicy | BackendTLSPolicy |
| edgiongatewayconfig | Edgion Gateway 配置 |

### apply - 应用配置

从 YAML 文件创建或更新资源。**仅 center target 支持此命令。**

```bash
edgion-cli apply -f <FILE|DIR> [-f <FILE|DIR> ...] [OPTIONS]
```

**选项:**
- `-f, --file <PATH>`: YAML 文件或目录路径（必需，可重复；按命令行顺序处理）
- `--dry-run`: 试运行，不实际应用

**示例:**

```bash
# 应用单个文件
edgion-cli apply -f route.yaml

# 应用目录下所有 YAML 文件
edgion-cli apply -f ./configs/

# 按顺序批量应用，并复用一个 CLI 运行时和客户端
edgion-cli apply -f gateway.yaml -f backend.yaml -f route.yaml

# 试运行
edgion-cli apply -f route.yaml --dry-run
```

### delete - 删除资源

删除指定资源。**仅 center target 支持此命令。**

```bash
edgion-cli delete <KIND> <NAME> [OPTIONS]
edgion-cli delete -f <FILE>
```

**选项:**
- `-n, --namespace <NS>`: 指定命名空间
- `-f, --file <PATH>`: 从 YAML 文件中读取要删除的资源

**示例:**

```bash
# 删除指定资源
edgion-cli delete httproute my-route -n default

# 从文件删除
edgion-cli delete -f route.yaml
```

### reload - 重新加载

从存储重新加载所有资源。**仅 center target 支持此命令。**

```bash
edgion-cli reload
```

### gen-certs - 生成 mTLS 证书

生成 conf_sync gRPC 通道 mTLS 所需的自签证书束。此命令不依赖运行中的 Controller 或 Gateway。

```bash
edgion-cli gen-certs [OPTIONS]
```

**选项:**

| 选项 | 说明 | 默认值 |
|------|------|--------|
| `--output` | 输出目标：`file`（写入文件）或 `k8s`（写入 K8s Secret） | `file` |
| `--output-dir` | 输出目录（仅 `--output file` 时生效） | `certs/grpc/` |
| `--cert-name` | 证书集名称，文件输出到 `<output-dir>/<cert-name>/` | `primary` |
| `--ca-days` | CA 证书有效天数 | `3650` |
| `--cert-days` | 服务端/客户端证书有效天数 | `365` |
| `--server-san` | 额外 SAN 条目（可多次指定） | 默认包含 localhost、127.0.0.1、::1 |
| `--force` | 强制覆盖已有证书 | `false` |

**生成的文件**（存放于 `<output-dir>/<cert-name>/`）：

```
<output-dir>/<cert-name>/
├── ca.crt        # CA 证书
├── ca.key        # CA 私钥（仅签发用）
├── server.crt    # Controller 服务端证书
├── server.key    # Controller 服务端私钥
├── client.crt    # Gateway 客户端证书
└── client.key    # Gateway 客户端私钥
```

**示例:**

```bash
# 生成到默认目录
edgion-cli gen-certs --output file

# 指定输出目录，强制覆盖
edgion-cli gen-certs --output file --output-dir /etc/edgion/certs --force

# 添加 K8s Service FQDN 作为 SAN
edgion-cli gen-certs --output file \
    --server-san edgion-controller.edgion-system.svc.cluster.local

# 写入 K8s Secret
edgion-cli gen-certs --output k8s
```

生成证书后，在 YAML 配置文件中配置 mTLS。详见[控制面 mTLS](infrastructure/mtls.md#控制面-mtlsconf_sync-grpc)。

### gen-token - 生成静态 CLI Token

生成新的角色范围 CLI Token，用于访问 Controller Admin API。不依赖运行中的 Controller。输出包含以 `edg_` 为前缀的 Token 及其 SHA-256 十六进制摘要。

```bash
edgion-cli gen-token
```

**输出示例：**

```
Token (use with --token or EDGION_TOKEN):
  edg_<URL-safe base64, 43 chars>

SHA-256 (paste into controller cli_tokens[].token_sha256):
  <64 lowercase hex chars>
```

**工作流程：**

1. 运行 `edgion-cli gen-token` 生成 Token。
2. 将 SHA-256 行粘贴到 Controller 配置文件的 `cli_tokens[].token_sha256` 中。
3. 热重载：`edgion-cli reload`（或 `POST /api/v1/reload`）。
4. 通过 Secret 管理器或环境变量将 `edg_...` Token 分发给消费方——切勿以明文存储于文件或 shell 历史记录中。
5. 消费方通过 `--token edg_...` 或 `export EDGION_TOKEN=edg_...` 使用 Token。

> **安全说明：** SHA-256 是对完整的 `edg_` 前缀 Token 计算的——即作为 `Authorization: Bearer` 值发送的完整字符串。Controller 配置中只存储 SHA-256 摘要，切勿存储原始 Token。

如需 RBAC 配置、轮换说明及所有安全注意事项，请参见[联邦 RBAC — CLI Token RBAC](federation-rbac.md#cli-token-rbac)。

### login - 持久化 Token

将 Bearer Token 保存到 `~/.edgion/token`，使后续命令无需 `--token` 或 `EDGION_TOKEN` 即可认证。这是纯 Token 方式——没有用户名/密码或 OIDC 登录。

```bash
edgion-cli login --token <TOKEN>
```

`<TOKEN>` 可以是本地管理员 Token（从 `edgion-admin-credentials` Secret 读取），也可以是 `gen-token` 生成的以 `edg_` 为前缀的 `cli_tokens` Token。从 Kubernetes 读取管理员 Token：

```bash
kubectl -n edgion-system get secret edgion-admin-credentials \
  -o jsonpath='{.data.token}' | base64 -d
```

仅当既未设置 `--token` 也未设置 `EDGION_TOKEN` 时才使用缓存的 Token（见[全局选项](#全局选项)下的优先级列表）。

## 连接配置

### 默认连接

根据 target 类型，`edgion-cli` 使用以下默认连接：

| Target | 默认地址 |
|--------|----------|
| center | `http://localhost:12101` |
| server | `http://localhost:12101` |
| client | `http://localhost:12001` |

### 自定义连接

使用 `--server` 选项指定服务器地址：

```bash
# 连接到远程 Controller
edgion-cli --server http://controller.example.com:12101 get httproute

# 连接到远程 Gateway
edgion-cli -t client --server http://gateway.example.com:12001 get service
```

## 输出格式

### table (默认)

以表格形式显示资源列表：

```
┌──────────────┬───────────┬───────────┐
│ NAME         │ NAMESPACE │ KIND      │
├──────────────┼───────────┼───────────┤
│ my-route     │ default   │ HTTPRoute │
│ api-route    │ prod      │ HTTPRoute │
└──────────────┴───────────┴───────────┘
```

### json

以 JSON 格式输出完整资源信息。

### yaml

以 YAML 格式输出完整资源信息。

### wide

扩展表格显示，包含更多字段。

## 故障排查

### 连接失败

如果连接失败，`edgion-cli` 会显示详细的错误信息和提示：

```
Error: Request to http://localhost:12101/api/v1/namespaced/httproute failed

Connection failed:
  - Is the controller running?
  - Check if the server address is correct
  - Try: curl -v http://localhost:12101/api/v1/namespaced/httproute

Hint: edgion-cli is trying to connect to: http://localhost:12101
      Target: Center (controller)
      Use --server to specify a different address
```

### 常见问题

1. **"apply command only supported for 'center' target"**
   
   `apply`、`delete`、`reload` 命令只能在 `center` target 下使用。`server` 和 `client` target 仅支持只读操作。

2. **资源未找到**
   
   检查资源名称、命名空间是否正确，以及 target 是否指向正确的组件。

3. **连接超时**
   
   确认目标服务正在运行，网络连接正常。

## 与 kubectl 的对比

| 操作 | kubectl | edgion-cli |
|------|---------|------------|
| 获取资源 | `kubectl get httproute` | `edgion-cli get httproute` |
| 指定命名空间 | `kubectl -n prod get httproute` | `edgion-cli get httproute -n prod` |
| 应用配置 | `kubectl apply -f route.yaml` | `edgion-cli apply -f route.yaml` |
| 删除资源 | `kubectl delete httproute my-route` | `edgion-cli delete httproute my-route` |
| 输出 YAML | `kubectl get httproute -o yaml` | `edgion-cli get httproute -o yaml` |

**区别:** `edgion-cli` 可以直接连接 Edgion 的 Admin API，无需 Kubernetes 集群，适用于文件系统模式部署。

## 相关文档

- [Gateway 资源总览](gateway/overview.md)
- [监控指标](observability/metrics.md)
- [Access Log 用户指南](observability/access-log.md)
- [控制面 mTLS](infrastructure/mtls.md)
