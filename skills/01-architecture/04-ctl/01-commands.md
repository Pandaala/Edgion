---
name: ctl-commands
description: edgion-cli 子命令详解：apply、get、delete、reload 的参数、实现逻辑、示例。
---

# edgion-cli 子命令

edgion-cli 提供 6 个子命令：apply（应用配置）、get（查询资源）、delete（删除资源）、reload（重载资源）、gen-certs（生成 mTLS 证书）、login（登录认证）。其中 apply、delete、reload 仅在 center target 下可用，gen-certs 不依赖 target。

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
| `--cert-name` | 证书集名称；文件写入 `<output-dir>/<cert-name>/`，K8s Secret 名称含此后缀 | `primary` |
| `--ca-days` | CA 证书有效天数 | `3650`（10 年） |
| `--cert-days` | 服务端/客户端证书有效天数 | `365` |
| `--server-san` | 额外 SAN（可多次指定） | 默认含 `localhost`、`127.0.0.1`、`::1` |
| `--force` | 强制覆盖已有证书 | `false` |

### 生成的文件

每次调用写入 `<output-dir>/<cert-name>/` 子目录，不同 `--cert-name` 之间相互独立：

```
<output-dir>/<cert-name>/
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
4. `--output file`：写入 PEM 文件到 `<output-dir>/<cert-name>/`
5. `--output k8s`：写入 K8s Secret，名称格式 `{prefix}-controller-tls-{cert-name}` / `{prefix}-gateway-tls-{cert-name}`

### 使用场景

- 本地开发/测试：快速生成自签证书启用 mTLS
- CI/CD：集成测试自动生成证书
- 生产环境初始化：一次性生成后交由证书管理系统轮转
- 证书轮换预置：生成 secondary 套件并预先写入配置，切换时只需改 `active` 字段

### 示例

```bash
# 生成 primary 证书集（默认名称）
edgion-cli gen-certs --output file --output-dir certs/grpc/

# 显式指定名称，等价上条
edgion-cli gen-certs --output file --output-dir certs/grpc/ --cert-name primary

# 预置 secondary 证书集（用于轮换）
edgion-cli gen-certs --output file --output-dir certs/grpc/ --cert-name secondary \
    --server-san edgion-controller.edgion-system.svc.cluster.local

# 指定输出目录，强制覆盖
edgion-cli gen-certs --output file --output-dir /etc/edgion/certs --cert-name primary --force

# 写入 K8s Secret（名称：{prefix}-controller-tls-primary 等）
edgion-cli gen-certs --output k8s --cert-name primary
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

## login — 登录认证

向 Controller 进行身份验证，token 保存到本地文件供后续命令自动携带。

### 语法

```bash
edgion-cli login -u <username> -p <password>
edgion-cli login  # 交互式输入
```

Token 保存到 `~/.edgion/token`，后续命令自动携带。

### 模块位置

| 模块 | 路径 |
|------|------|
| CLI 自动认证逻辑 | `src/core/ctl/cli/auto_auth.rs` |
| Token 文件管理 | `src/core/ctl/cli/token.rs` |

## 自动认证（ensure_auth）

edgion-cli 在执行命令前自动检测认证状态，无需手动 login：

1. 已有 token → 直接使用
2. 查询 `/api/v1/auth/status` → 不需要认证则跳过
3. 自动检测 Controller 配置文件（`--controller-config` / `EDGION_CONTROLLER_CONFIG` / 默认路径）
4. 从配置推断 conf_dir → 读取 Secret `edgion-admin-credentials` → 自动登录
5. 检测失败 → 提示手动 `edgion-cli login`

### Controller vs Center 非对称性

- **Controller**：Step 4 总会命中——Controller 启动时会自动生成 Secret
  `edgion-admin-credentials`，`ensure_auth` 直接读取即可
- **Center**：没有 Secret 自动生成路径。operator 必须在 Center YAML 中
  显式配置 `[local_auth]` 或 `[auth]`；未配置时 Center 所有业务路由返回 503，
  CLI 无法 auto-login（需手动 `edgion-cli login` 并自带凭证）。
  详见 `skills/02-features/02-config/03-auth-bootstrap.md`

### `/api/v1/auth/status` 永远可达

`/api/v1/auth/status` 端点在 `compose_admin_routes` 中无条件挂载（Controller 与 Center
共享同一装配入口；fix #01 round 3 回归点）。即便 Center 在零配置下所有业务路由 503，
`/api/v1/auth/status` 仍返回 200，CLI `ensure_auth` 可以正常查询认证模式。

### 配置文件自动检测路径

| 优先级 | 来源 |
|--------|------|
| 1 | `--controller-config` / `-C` 参数 |
| 2 | `EDGION_CONTROLLER_CONFIG` 环境变量 |
| 3 | `config/edgion-controller.yaml`（相对 CWD）|
| 4 | `/usr/local/edgion/config/edgion-controller.yaml` |
| 5 | `/etc/edgion/edgion-controller.yaml` |
