# edgion-ctl 命令行工具

`edgion-ctl` 是 Edgion 的命令行管理工具，用于查看和管理网关资源配置。

## 安装

编译后的二进制文件位于 `target/release/edgion-ctl` 或 `target/debug/edgion-ctl`。

## 基本用法

```bash
edgion-ctl [OPTIONS] <COMMAND>
```

### 全局选项

| 选项 | 简写 | 说明 | 默认值 |
|------|------|------|--------|
| `--target` | `-t` | 目标 API 类型 | `center` |
| `--server` | - | 服务器地址 | 根据 target 自动选择 |
| `--socket` | - | Unix socket 路径 | - |
| `--help` | `-h` | 显示帮助信息 | - |
| `--version` | `-V` | 显示版本信息 | - |

## Target 类型

`edgion-ctl` 支持三种 target 类型，用于连接不同的数据源：

| Target | 组件 | 默认端口 | 支持的命令 | 说明 |
|--------|------|----------|------------|------|
| `center` | Controller/ConfCenter | 5800 | get, apply, delete, reload | 完整 CRUD 操作 |
| `server` | Controller/ConfigServer | 5800 | get (只读) | 查看 ConfigServer 缓存 |
| `client` | Gateway/ConfigClient | 5900 | get (只读) | 查看 Gateway 缓存 |

### 使用示例

```bash
# center (默认) - 完整功能
edgion-ctl get httproute
edgion-ctl apply -f route.yaml
edgion-ctl delete httproute my-route -n default

# server - 查看 Controller 的 ConfigServer 缓存
edgion-ctl -t server get httproute
edgion-ctl -t server get httproute -n prod

# client - 查看 Gateway 的 ConfigClient 缓存
edgion-ctl -t client get httproute
edgion-ctl -t client --server http://gateway:5900 get service -n default
```

## 命令详解

### get - 获取资源

获取单个资源或列出资源列表。

```bash
edgion-ctl get <KIND> [NAME] [OPTIONS]
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
edgion-ctl get httproute

# 列出指定命名空间的 HTTPRoute
edgion-ctl get httproute -n production

# 获取指定资源，输出 YAML 格式
edgion-ctl get httproute my-route -n default -o yaml

# 获取指定资源，输出 JSON 格式
edgion-ctl get service backend-svc -n default -o json
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
edgion-ctl apply -f <FILE|DIR> [OPTIONS]
```

**选项:**
- `-f, --file <PATH>`: YAML 文件或目录路径（必需）
- `--dry-run`: 试运行，不实际应用

**示例:**

```bash
# 应用单个文件
edgion-ctl apply -f route.yaml

# 应用目录下所有 YAML 文件
edgion-ctl apply -f ./configs/

# 试运行
edgion-ctl apply -f route.yaml --dry-run
```

### delete - 删除资源

删除指定资源。**仅 center target 支持此命令。**

```bash
edgion-ctl delete <KIND> <NAME> [OPTIONS]
edgion-ctl delete -f <FILE>
```

**选项:**
- `-n, --namespace <NS>`: 指定命名空间
- `-f, --file <PATH>`: 从 YAML 文件中读取要删除的资源

**示例:**

```bash
# 删除指定资源
edgion-ctl delete httproute my-route -n default

# 从文件删除
edgion-ctl delete -f route.yaml
```

### reload - 重新加载

从存储重新加载所有资源。**仅 center target 支持此命令。**

```bash
edgion-ctl reload
```

## 连接配置

### 默认连接

根据 target 类型，`edgion-ctl` 使用以下默认连接：

| Target | 默认地址 |
|--------|----------|
| center | http://localhost:5800 |
| server | http://localhost:5800 |
| client | http://localhost:5900 |

### 自定义连接

使用 `--server` 选项指定服务器地址：

```bash
# 连接到远程 Controller
edgion-ctl --server http://controller.example.com:5800 get httproute

# 连接到远程 Gateway
edgion-ctl -t client --server http://gateway.example.com:5900 get service
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

如果连接失败，`edgion-ctl` 会显示详细的错误信息和提示：

```
Error: Request to http://localhost:5800/api/v1/namespaced/httproute failed

Connection failed:
  - Is the controller running?
  - Check if the server address is correct
  - Try: curl -v http://localhost:5800/api/v1/namespaced/httproute

Hint: edgion-ctl is trying to connect to: http://localhost:5800
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

| 操作 | kubectl | edgion-ctl |
|------|---------|------------|
| 获取资源 | `kubectl get httproute` | `edgion-ctl get httproute` |
| 指定命名空间 | `kubectl -n prod get httproute` | `edgion-ctl get httproute -n prod` |
| 应用配置 | `kubectl apply -f route.yaml` | `edgion-ctl apply -f route.yaml` |
| 删除资源 | `kubectl delete httproute my-route` | `edgion-ctl delete httproute my-route` |
| 输出 YAML | `kubectl get httproute -o yaml` | `edgion-ctl get httproute -o yaml` |

**区别:** `edgion-ctl` 可以直接连接 Edgion 的 Admin API，无需 Kubernetes 集群，适用于文件系统模式部署。
