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

`--token` 接受由 `gen-token` 生成的以 `edg_` 为前缀的 Token。环境变量 `EDGION_TOKEN` 是避免 Token 暴露在 shell 历史记录中的替代方式。优先级：`--token` > `EDGION_TOKEN` > 缓存的登录 Token。设置任一方式后，`ensure_auth`（包括 `/auth/status` 和 `/auth/me`）将被跳过；验证为惰性（401 = 未知 Token，403 = 权限不足）。

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
| pluginmetadata | 插件元数据 |
| linksys | LinkSys 配置 |
| referencegrant | ReferenceGrant |
| backendtlspolicy | BackendTLSPolicy |
| edgiongatewayconfig | Edgion Gateway 配置 |

### apply - 应用配置

从 YAML 文件创建或更新资源。**仅 center target 支持此命令。**

```bash
edgion-cli apply -f <FILE|DIR> [OPTIONS]
```

**选项:**
- `-f, --file <PATH>`: YAML 文件或目录路径（必需）
- `--dry-run`: 试运行，不实际应用

**示例:**

```bash
# 应用单个文件
edgion-cli apply -f route.yaml

# 应用目录下所有 YAML 文件
edgion-cli apply -f ./configs/

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

## 连接配置

### 默认连接

根据 target 类型，`edgion-cli` 使用以下默认连接：

| Target | 默认地址 |
|--------|----------|
| center | http://localhost:12101 |
| server | http://localhost:12101 |
| client | http://localhost:12001 |

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
