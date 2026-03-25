---
name: deployment-patterns
description: Edgion 常见部署模式：FileSystem 本地开发、K8s 生产部署、HA 模式。
---

# 部署模式

## 模式总览

| 模式 | conf_center | 适用场景 | 特点 |
|------|-------------|---------|------|
| FileSystem | `type = "filesystem"` | 本地开发/测试/CI | YAML 文件驱动，无需 K8s |
| Kubernetes 单实例 | `type = "kubernetes"` | 小规模/测试 K8s | 直接 watch K8s API |
| Kubernetes HA | `type = "kubernetes"` + `ha_mode = true` | 生产环境 | Leader 选举，多副本高可用 |

## FileSystem 模式

```
                edgion-controller              edgion-gateway
                (FileSystem ConfCenter)        (数据面)
  YAML Files ──► conf_dir ──► 处理 ──► gRPC ──► 代理流量
```

### 配置示例

```toml
# edgion-controller.toml
[conf_center]
type = "filesystem"
conf_dir = "config/resources"     # YAML 资源目录
endpoint_mode = "EndpointSlice"   # EndpointSlice | Endpoints | Both
```

### 适用场景
- 本地开发和调试
- CI/CD 集成测试
- 不依赖 K8s 的独立部署

### 典型启动

```bash
# Terminal 1: Controller
edgion-controller --conf-dir config/resources

# Terminal 2: Gateway
edgion-gateway --server-addr http://127.0.0.1:50051
```

---

## Kubernetes 模式

```
  K8s API Server
       │ watch/list
       ▼
  edgion-controller (KubernetesCenter)
       │ gRPC
       ▼
  edgion-gateway × N (数据面 Pod)
```

### 配置示例

```toml
# edgion-controller-k8s.toml
[conf_center]
type = "kubernetes"
gateway_class = "edgion"          # 必填：匹配的 GatewayClass 名称
endpoint_mode = "EndpointSlice"

# 可选：命名空间过滤
watch_namespaces = ["default", "production"]

# 可选：标签过滤
[conf_center.metadata_filter]
label_selector = "app=edgion"
```

### 关键字段

| 字段 | 类型 | 默认 | 说明 |
|------|------|------|------|
| `gateway_class` | `String` | **必填** | 只处理此 GatewayClass 的资源 |
| `watch_namespaces` | `Vec<String>` | 全部 | 限制 watch 的命名空间列表 |
| `endpoint_mode` | `String` | `EndpointSlice` | 后端发现模式 |
| `gateway_address` | `String?` | — | Gateway status 中报告的地址 |

---

## Kubernetes HA 模式

```
  edgion-controller-0 (Leader)  ◄── 写入 status、推送 gRPC
  edgion-controller-1 (Standby) ◄── 等待 Leader 租约过期
  edgion-controller-2 (Standby)
       │ gRPC (仅 Leader)
       ▼
  edgion-gateway × N
```

### 配置示例

```toml
[conf_center]
type = "kubernetes"
gateway_class = "edgion"
ha_mode = true

[conf_center.leader_election]
lease_name = "edgion-controller-leader"
lease_namespace = "edgion-system"
lease_duration_seconds = 15
renew_deadline_seconds = 10
retry_period_seconds = 2
```

### Leader 选举字段 Schema

| 字段 | 类型 | 默认 | 说明 |
|------|------|------|------|
| `lease_name` | `String` | `"edgion-controller-leader"` | Lease 资源名称 |
| `lease_namespace` | `String` | `"edgion-system"` | Lease 资源命名空间 |
| `lease_duration_seconds` | `u64` | `15` | 租约持续时间 |
| `renew_deadline_seconds` | `u64` | `10` | 续约截止时间 |
| `retry_period_seconds` | `u64` | `2` | 重试间隔 |

### HA 行为
- **仅 Leader** 执行：资源处理、status 回写、ACME 证书签发、gRPC 推送
- **Standby** 行为：watch K8s 资源但不处理，等待 Leader 故障转移
- **故障转移**：Leader 租约过期后 Standby 竞争成为新 Leader，重新全量处理

---

## 部署拓扑建议

| 场景 | Controller | Gateway | 建议 |
|------|-----------|---------|------|
| 开发环境 | 1 (FileSystem) | 1 | 单进程，快速迭代 |
| 测试环境 | 1 (K8s) | 1-2 | 验证 K8s 集成 |
| 生产环境 | 3 (K8s HA) | 2+ | 高可用，Gateway 按需扩缩 |

### Gateway 扩缩
- Gateway 是**无状态**的数据面，可自由水平扩缩
- 每个 Gateway Pod 独立从 Controller 获取配置
- 通过 K8s Service/LoadBalancer 做流量分发
