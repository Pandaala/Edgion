---
name: ctl-commands
description: edgion-cli 子命令详解：apply、get、delete、reload 的参数、实现逻辑、示例。
---

# edgion-cli 子命令

edgion-cli 提供 5 个子命令：apply（应用配置）、get（查询资源）、delete（删除资源）、reload（重载资源）、gen-certs（生成 mTLS 证书）。其中 apply、delete、reload 仅在 center target 下可用，gen-certs 不依赖 target。

## apply

从文件或目录应用配置资源，仅支持 center target。

### 语法

```bash
edgion-cli apply -f <file|directory> [--dry-run]
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
edgion-cli apply -f gateway.yaml

# 应用目录下所有 YAML
edgion-cli apply -f manifests/

# Dry run 验证
edgion-cli apply -f gateway.yaml --dry-run
```

## get

查询资源，支持所有三种 target 模式。

### 语法

```bash
edgion-cli [-t <target>] get <kind> [name] [-n <namespace>] [-o <format>]
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
edgion-cli get httproute

# 按命名空间过滤
edgion-cli get httproute -n production

# 获取指定资源的 YAML
edgion-cli get httproute my-route -n default -o yaml

# 查看 Gateway 缓存中的资源（JSON 格式）
edgion-cli -t client get httproute -o json

# 宽表格显示更多信息
edgion-cli get gateway -o wide
```

## delete

删除资源，仅支持 center target。支持两种删除方式：按 kind/name 指定或从文件读取。

### 语法

```bash
# 方式 1：按 kind/name 删除
edgion-cli delete <kind> <name> [-n <namespace>]

# 方式 2：从文件删除
edgion-cli delete -f <file>
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
edgion-cli delete httproute my-route -n default

# 从文件删除
edgion-cli delete -f gateway.yaml
```

## reload

从存储重新加载所有资源，仅支持 center target。触发 Controller 全量重处理。

### 语法

```bash
edgion-cli reload
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
edgion-cli reload

# 连接指定地址的 Controller
edgion-cli --server http://10.0.0.1:5800 reload
```

## gen-certs

生成 conf_sync gRPC mTLS 所需的证书束（CA + Server + Client），基于 rcgen 库实现。此命令不依赖 target，不需要连接任何服务。

### 语法

```bash
edgion-cli gen-certs [OPTIONS]
```

### 参数

| 参数 | 说明 | 默认 |
|------|------|------|
| `--output` | 输出目标：`file`（写入文件）或 `k8s`（写入 K8s Secret） | `file` |
| `--output-dir` | 文件输出目录（仅 `--output file` 时生效） | `certs/grpc/` |
| `--ca-days` | CA 证书有效天数 | `3650`（10 年） |
| `--cert-days` | 服务端/客户端证书有效天数 | `365` |
| `--server-san` | 额外 SAN（可多次指定） | 默认含 `localhost`、`127.0.0.1`、`::1` |
| `--force` | 强制覆盖已有证书 | `false` |

### 生成的文件

```
<output-dir>/
├── ca.crt            # CA 证书（分发给双方校验对端）
├── ca.key            # CA 私钥（仅签发用，不部署到运行时）
├── server.crt        # Controller 服务端证书
├── server.key        # Controller 服务端私钥
├── client.crt        # Gateway 客户端证书
└── client.key        # Gateway 客户端私钥
```

### 证书属性

| 证书 | CN | SAN | EKU |
|------|----|----|-----|
| CA | edgion-grpc-ca | — | — |
| Server | edgion-controller | localhost, 127.0.0.1, ::1, + `--server-san` | ServerAuth |
| Client | edgion-gateway | — | ClientAuth |

### 实现逻辑

1. 使用 rcgen 创建自签 CA（ECDSA P-256）
2. 用 CA 签发 Server 证书（含 SAN 和 ServerAuth EKU）
3. 用 CA 签发 Client 证书（含 ClientAuth EKU）
4. `--output file`：写入 PEM 文件到 `--output-dir`
5. `--output k8s`：写入 K8s Secret（用于 Helm/Operator 部署）

### 使用场景

- 本地开发/测试：快速生成自签证书启用 mTLS
- CI/CD：集成测试自动生成证书
- 生产环境初始化：一次性生成后交由证书管理系统轮转

### 示例

```bash
# 生成到默认目录 certs/grpc/
edgion-cli gen-certs --output file

# 指定输出目录，强制覆盖
edgion-cli gen-certs --output file --output-dir /etc/edgion/certs --force

# 添加 K8s Service FQDN 作为额外 SAN
edgion-cli gen-certs --output file \
    --server-san edgion-controller.edgion-system.svc.cluster.local

# 写入 K8s Secret
edgion-cli gen-certs --output k8s
```

### 模块结构

```
src/core/ctl/
├── certs/
│   └── mod.rs                # generate_certs() → CertBundle（CA、Server、Client）
└── cli/
    └── commands/
        └── gen_certs.rs      # GenCertsArgs、write_to_files()、write_to_k8s_secrets()
```
