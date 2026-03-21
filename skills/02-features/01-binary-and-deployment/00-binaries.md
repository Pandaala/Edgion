---
name: binaries
description: 三个 bin 的入口点、CLI 参数完整参考、work_dir 优先级规则。
---

# 二进制入口与 CLI 参数

## 概览

| Binary | 用途 | 默认端口 | 入口 |
|--------|------|---------|------|
| `edgion-controller` | 控制面：资源接收/校验/处理/分发 | gRPC `:50051`, Admin `:5800` | `src/bin/edgion_controller.rs` |
| `edgion-gateway` | 数据面：高性能代理（基于 Pingora） | Admin `:5900`, Metrics `:5901` | `src/bin/edgion_gateway.rs` |
| `edgion-ctl` | CLI 工具：资源 CRUD 和运维 | — | `src/bin/edgion_ctl.rs` |

三个 bin 都采用**薄入口点**模式 — `src/bin/*.rs` 只做最少的工作：安装 rustls 加密提供程序 → 解析参数 → `run()`。

## work_dir 优先级

```
CLI --work-dir > ENV EDGION_WORK_DIR > TOML config > Default (".")
```

所有配置中的相对路径都基于 `work_dir` 解析。

---

## edgion-controller CLI

```
edgion-controller [OPTIONS]
```

### 参数 Schema

| 参数 | 短 | 类型 | 默认值 | 说明 |
|------|---|------|--------|------|
| `--work-dir` | `-w` | `PathBuf` | `"."` | 工作目录 |
| `--config-file` | `-c` | `String` | `config/edgion-controller.toml` | TOML 配置文件路径 |
| `--conf-dir` | — | `PathBuf` | — | FileSystem 模式的配置目录（覆盖 TOML 中 conf_center.conf_dir） |
| `--test-mode` | — | `bool` | `false` | 启用测试模式（endpoint_mode=Both + metrics 测试功能） |
| `--grpc-listen` | — | `String` | `0.0.0.0:50051` | gRPC 监听地址 |
| `--admin-listen` | — | `String` | `0.0.0.0:5800` | Admin HTTP API 地址 |
| `--log-dir` | — | `String` | `logs` | 日志目录 |
| `--log-level` | — | `String` | `info` | 日志级别：trace/debug/info/warn/error |
| `--json-format` | — | `bool` | `false` | JSON 格式日志 |
| `--console` | — | `bool` | `true` | 控制台输出 |

### 启动示例

```bash
# FileSystem 模式（本地开发）
edgion-controller -w /opt/edgion --conf-dir config/resources

# K8s 模式（生产）
edgion-controller -c config/edgion-controller-k8s.toml
```

---

## edgion-gateway CLI

```
edgion-gateway [OPTIONS]
```

### 参数 Schema

| 参数 | 短 | 类型 | 默认值 | 说明 |
|------|---|------|--------|------|
| `--work-dir` | `-w` | `PathBuf` | `"."` | 工作目录 |
| `--config-file` | `-c` | `String` | `config/edgion-gateway.toml` | TOML 配置文件路径 |
| `--server-addr` | — | `String` | **必填** | Controller gRPC 地址（如 `http://127.0.0.1:50051`） |
| `--admin-listen` | — | `String` | — | Admin API 地址（注意：当前固定为 `:5900`） |
| `--threads` | — | `usize` | CPU 核心数 | Pingora 工作线程数 |
| `--work-stealing` | — | `bool` | `true` | Tokio 任务窃取 |
| `--grace-period` | — | `u64` | `30` | 优雅关闭等待秒数 |
| `--graceful-shutdown-timeout` | — | `u64` | `10` | 关闭超时秒数 |
| `--upstream-keepalive-pool-size` | — | `usize` | `128` | 上游连接池大小 |
| `--downstream-keepalive-request-limit` | — | `u32` | `1000` | 下游每连接最大请求数（0=无限） |
| `--error-log` | — | `String` | — | Pingora 错误日志路径 |
| `--log-dir` | — | `String` | `logs` | 系统日志目录 |
| `--log-level` | — | `String` | `info` | 日志级别 |
| `--json-format` | — | `bool` | `false` | JSON 格式日志 |
| `--integration-testing-mode` | — | `bool` | `false` | 集成测试模式（**禁止生产使用**） |

### 启动示例

```bash
# 连接本地 Controller
edgion-gateway --server-addr http://127.0.0.1:50051

# 生产部署
edgion-gateway -c config/edgion-gateway.toml --server-addr http://controller:50051 --threads 8
```

---

## edgion-ctl CLI

```
edgion-ctl [OPTIONS] <COMMAND>
```

### 全局参数

| 参数 | 短 | 类型 | 默认值 | 说明 |
|------|---|------|--------|------|
| `--target` | `-t` | `center\|server\|client` | `center` | 目标 API |
| `--server` | — | `String` | — | 服务地址（如 `http://localhost:5800`） |
| `--socket` | — | `PathBuf` | — | Unix socket 路径 |

### Target 模式

| Target | 连接端口 | 操作 | 典型场景 |
|--------|---------|------|---------|
| `center` | Controller `:5800` | 完整 CRUD | 资源管理（默认） |
| `server` | Controller `:5800` | 只读查询 | 查看 ServerCache 状态 |
| `client` | Gateway `:5900` | 只读查询 | 查看 Gateway 侧缓存 |

### 子命令

```bash
# 应用资源
edgion-ctl apply -f route.yaml
edgion-ctl apply -f config/resources/    # 目录

# 查询资源
edgion-ctl get httproute                 # 列出所有
edgion-ctl get httproute my-route -n default -o yaml

# 删除资源
edgion-ctl delete httproute my-route -n default
edgion-ctl delete -f route.yaml

# 重载
edgion-ctl reload

# 查看 Gateway 侧缓存
edgion-ctl -t client --server http://localhost:5900 get httproute
```

### 子命令参数 Schema

**apply**:
| 参数 | 短 | 类型 | 说明 |
|------|---|------|------|
| `--file` | `-f` | `String` | 文件或目录路径 |
| `--dry-run` | — | `bool` | 模拟执行，不实际变更 |

**get**:
| 参数 | 位置 | 类型 | 说明 |
|------|------|------|------|
| `kind` | 1st | `String` | 资源类型（如 httproute、gateway） |
| `name` | 2nd | `String?` | 资源名称（省略则列出全部） |
| `--namespace` / `-n` | — | `String?` | 命名空间 |
| `--output` / `-o` | — | `String` | 输出格式：table/json/yaml/wide |

**delete**:
| 参数 | 位置 | 类型 | 说明 |
|------|------|------|------|
| `kind` | 1st | `String?` | 资源类型 |
| `name` | 2nd | `String?` | 资源名称 |
| `--namespace` / `-n` | — | `String?` | 命名空间 |
| `--file` / `-f` | — | `String?` | 按文件删除 |
