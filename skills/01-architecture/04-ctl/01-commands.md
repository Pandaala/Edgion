---
name: ctl-commands
description: edgion-ctl 子命令详解：apply、get、delete、reload 的参数、实现逻辑、示例。
---

# edgion-ctl 子命令

edgion-ctl 提供 4 个子命令：apply（应用配置）、get（查询资源）、delete（删除资源）、reload（重载资源）。其中 apply、delete、reload 仅在 center target 下可用。

## apply

从文件或目录应用配置资源，仅支持 center target。

### 语法

```bash
edgion-ctl apply -f <file|directory> [--dry-run]
```

### 参数

| 参数 | 说明 |
|------|------|
| `-f, --file` | YAML 文件路径或包含 YAML 文件的目录 |
| `--dry-run` | 仅验证配置，不实际写入 |

### 实现逻辑

1. 判断路径类型：文件 → 单文件处理；目录 → 遍历目录下所有 `.yaml`/`.yml` 文件
2. 读取 YAML 内容，解析 `kind`、`metadata.name`、`metadata.namespace`
3. 判断资源是否已存在：
   - 不存在 → 调用 `POST /api/v1/namespaced/{kind}/{ns}` 创建
   - 已存在 → 调用 `PUT /api/v1/namespaced/{kind}/{ns}/{name}` 更新
4. dry-run 模式下添加查询参数 `?dryRun=true`，仅执行验证不写入

### 示例

```bash
# 应用单个文件
edgion-ctl apply -f gateway.yaml

# 应用目录下所有 YAML
edgion-ctl apply -f manifests/

# Dry run 验证
edgion-ctl apply -f gateway.yaml --dry-run
```

## get

查询资源，支持所有三种 target 模式。

### 语法

```bash
edgion-ctl [-t <target>] get <kind> [name] [-n <namespace>] [-o <format>]
```

### 参数

| 参数 | 说明 |
|------|------|
| `kind` | 资源类型（如 httproute、gateway、service），大小写不敏感 |
| `name` | 资源名称（可选，不指定时列出全部） |
| `-n, --namespace` | 命名空间过滤 |
| `-o, --output` | 输出格式：table（默认）、json、yaml、wide |

### 查询逻辑

根据 target 不同，get 命令的查询路径不同：

| Target | 有 name | 无 name |
|--------|---------|---------|
| center | `GET /api/v1/namespaced/{kind}/{ns}/{name}` 或 `GET /api/v1/cluster/{kind}/{name}` | `GET /api/v1/namespaced/{kind}` 或按 namespace 过滤 |
| server | `GET /configserver/{kind}?name=N&namespace=NS` | `GET /configserver/{kind}/list` |
| client | `GET /configclient/{kind}?name=N&namespace=NS` | `GET /configclient/{kind}/list` |

server/client 模式下的命名空间过滤是客户端侧完成的：先获取全量数据，再按 namespace 筛选。

### 示例

```bash
# 列出所有 HTTPRoute（center 模式）
edgion-ctl get httproute

# 按命名空间过滤
edgion-ctl get httproute -n production

# 获取指定资源的 YAML
edgion-ctl get httproute my-route -n default -o yaml

# 查看 Gateway 缓存中的资源（JSON 格式）
edgion-ctl -t client get httproute -o json

# 宽表格显示更多信息
edgion-ctl get gateway -o wide
```

## delete

删除资源，仅支持 center target。支持两种删除方式：按 kind/name 指定或从文件读取。

### 语法

```bash
# 方式 1：按 kind/name 删除
edgion-ctl delete <kind> <name> [-n <namespace>]

# 方式 2：从文件删除
edgion-ctl delete -f <file>
```

### 参数

| 参数 | 说明 |
|------|------|
| `kind` | 资源类型（与 -f 互斥） |
| `name` | 资源名称（与 -f 互斥） |
| `-n, --namespace` | 命名空间 |
| `-f, --file` | 从 YAML 文件读取要删除的资源 |

### 实现逻辑

- **按 kind/name 删除**：直接调用 `DELETE /api/v1/namespaced/{kind}/{ns}/{name}` 或 `DELETE /api/v1/cluster/{kind}/{name}`
- **从文件删除**：解析 YAML 文件中的 `kind`、`metadata.name`、`metadata.namespace`，然后调用对应的 DELETE API

### 示例

```bash
# 按 kind/name 删除
edgion-ctl delete httproute my-route -n default

# 从文件删除
edgion-ctl delete -f gateway.yaml
```

## reload

从存储重新加载所有资源，仅支持 center target。触发 Controller 全量重处理。

### 语法

```bash
edgion-ctl reload
```

### 实现逻辑

调用 `POST /api/v1/reload`，Controller 收到请求后：

1. 清空所有 ServerCache（`reset_for_relink`）
2. 生成新的 `server_id`
3. 重新从 ConfCenter（File/K8s）读取所有资源
4. 重新执行全量处理流水线
5. Gateway 检测到 `server_id` 变化后自动全量 relist

### 使用场景

- Controller 与外部存储状态不一致时强制重同步
- 开发环境下手动触发全量重处理
- 排查资源处理流水线问题

### 示例

```bash
# 重载所有资源
edgion-ctl reload

# 连接指定地址的 Controller
edgion-ctl --server http://10.0.0.1:5800 reload
```
