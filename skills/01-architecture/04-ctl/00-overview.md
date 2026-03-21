---
name: ctl-overview
description: edgion-ctl 总览：三种 Target 模式（center/server/client）、EdgionClient、API 路由、调试用法。
---

# edgion-ctl 总览

edgion-ctl 是 Edgion 的命令行管理工具，基于 clap 构建。通过 HTTP API 与 Controller 和 Gateway 交互，支持资源 CRUD、缓存查询和运维操作。其核心价值在于可以对比三级缓存（ConfCenter → ServerCache → ClientCache）排查配置同步问题。

## 三种 Target 模式

| Target | 连接目标 | 默认端口 | 能力 | 数据源 |
|--------|---------|---------|------|--------|
| **center**（默认） | Controller Admin API | :5800 | 完整 CRUD + reload | ConfCenter（配置中心） |
| **server** | Controller Admin API | :5800 | 只读查询 | ServerCache（gRPC 服务端缓存） |
| **client** | Gateway Admin API | :5900 | 只读查询 | ClientCache（gRPC 客户端缓存） |

```
                    edgion-ctl
                   ┌──────────┐
                   │ -t center│──── HTTP :5800 ──► Controller ConfCenter (read/write)
                   │ -t server│──── HTTP :5800 ──► Controller ServerCache (read-only)
                   │ -t client│──── HTTP :5900 ──► Gateway    ClientCache (read-only)
                   └──────────┘
```

### Target 与命令的权限关系

| 命令 | center | server | client |
|------|--------|--------|--------|
| get | 支持 | 支持 | 支持 |
| apply | 支持 | 不支持 | 不支持 |
| delete | 支持 | 不支持 | 不支持 |
| reload | 支持 | 不支持 | 不支持 |

写操作（apply/delete/reload）仅在 center 模式下可用。server 和 client 模式为只读，用于查看各级缓存的状态。

## API 路由

不同 target 对应不同的 HTTP API 路由：

### center 路由（ConfCenter API）

```
GET    /api/v1/namespaced/{kind}              # 列出所有命名空间下的资源
GET    /api/v1/namespaced/{kind}/{namespace}   # 列出指定命名空间下的资源
GET    /api/v1/namespaced/{kind}/{ns}/{name}   # 获取指定资源
GET    /api/v1/cluster/{kind}/{name}           # 获取集群级资源
POST   /api/v1/namespaced/{kind}/{namespace}   # 创建命名空间资源
POST   /api/v1/cluster/{kind}                  # 创建集群级资源
PUT    /api/v1/namespaced/{kind}/{ns}/{name}   # 更新命名空间资源
PUT    /api/v1/cluster/{kind}/{name}           # 更新集群级资源
DELETE /api/v1/namespaced/{kind}/{ns}/{name}   # 删除命名空间资源
DELETE /api/v1/cluster/{kind}/{name}           # 删除集群级资源
POST   /api/v1/reload                          # 重载所有资源
```

### server 路由（ConfigServer 缓存）

```
GET    /configserver/{kind}/list               # 列出所有资源
GET    /configserver/{kind}?name=N&namespace=NS  # 按名称/命名空间过滤
```

### client 路由（ConfigClient 缓存）

```
GET    /configclient/{kind}/list               # 列出所有资源
GET    /configclient/{kind}?name=N&namespace=NS  # 按名称/命名空间过滤
```

server 和 client 路由的过滤是客户端侧完成的——先获取全量数据，再在 edgion-ctl 内部过滤。

## EdgionClient

HTTP 客户端实现，封装了与 Controller/Gateway 的 HTTP 通信逻辑。

```rust
pub struct EdgionClient {
    client: Client,         // reqwest HTTP 客户端
    base_url: String,       // 基础 URL（如 http://localhost:5800）
    target: TargetType,     // 目标类型
    socket_path: Option<PathBuf>,  // Unix socket 路径（可选）
}
```

### 连接方式

- **HTTP**：默认方式，根据 target 类型选择端口（center/server → 5800，client → 5900）
- **Unix Socket**：通过 `--socket` 参数指定，适用于同一节点上的本地通信

### 错误处理

EdgionClient 提供详细的网络错误诊断：
- 连接拒绝 → 提示对应组件（controller/gateway）可能未运行
- DNS 解析失败 → 提示检查主机名
- 超时 → 提示服务可能过载
- 通用连接错误 → 提示检查服务器地址

## 输出格式

| 格式 | 说明 |
|------|------|
| `table`（默认） | 表格形式，显示关键字段 |
| `json` | 完整 JSON 输出，适合程序化处理 |
| `yaml` | 完整 YAML 输出，与配置文件格式一致 |
| `wide` | 宽表格，显示更多字段 |

通过 `-o` 参数指定：`edgion-ctl get httproute -o json`

## 调试用法

edgion-ctl 最有价值的用途是排查配置同步问题。通过对比三级缓存可以快速定位问题所在层级：

### 场景 1：配置已 apply 但 Gateway 未生效

```bash
# 1. 确认 center 是否已写入
edgion-ctl get httproute my-route -n default -o yaml

# 2. 确认 server 缓存是否已更新
edgion-ctl -t server get httproute my-route -n default -o yaml

# 3. 确认 client 缓存是否已同步
edgion-ctl -t client get httproute my-route -n default -o yaml
```

- center 有但 server 没有 → 问题在 ResourceProcessor 处理阶段
- server 有但 client 没有 → 问题在 gRPC 同步阶段
- client 有但未生效 → 问题在 ConfHandler / preparse 阶段

### 场景 2：检查同步状态

```bash
# 对比 server 和 client 的资源数量
edgion-ctl -t server get httproute -o json | jq length
edgion-ctl -t client get httproute -o json | jq length
```

## CLI 参数总览

| 参数 | 短选项 | 说明 |
|------|--------|------|
| `--target` | `-t` | Target 模式：center（默认）、server、client |
| `--server` | | 服务器地址（如 `http://10.0.0.1:5800`） |
| `--socket` | | Unix socket 路径 |
| `--namespace` | `-n` | 命名空间过滤 |
| `--output` | `-o` | 输出格式：table、json、yaml、wide |
| `--file` | `-f` | 文件/目录路径（apply/delete 用） |
| `--dry-run` | | Dry run 模式（仅 apply） |

## 模块结构

```
src/core/ctl/
├── cli/
│   ├── mod.rs              # Cli 结构、TargetType、Commands 枚举、命令路由
│   ├── client.rs           # EdgionClient HTTP 客户端实现
│   ├── output.rs           # OutputFormat、表格/JSON/YAML 输出格式化
│   └── commands/
│       ├── mod.rs          # 子命令模块导出
│       ├── apply.rs        # apply 命令（YAML 解析 → API 调用）
│       ├── get.rs          # get 命令（查询 + 格式化输出）
│       ├── delete.rs       # delete 命令
│       └── reload.rs       # reload 命令
└── mod.rs
```
