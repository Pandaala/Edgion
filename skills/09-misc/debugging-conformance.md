# Conformance 测试调试指南

本文档说明如何在 conformance 测试失败时快速定位问题，覆盖日志收集、rv/sv 追踪、常见失败模式识别。

## 关键前提：日志在文件，不在 stdout

`kubectl logs`（`orbstack-dev.sh logs`）只能看进程 stdout，那里只有极少量输出（进程崩溃 / panic）。
所有有价值的日志在 pod 内文件：

| 日志 | Pod 内路径 | 含义 |
|------|-----------|------|
| gateway 系统日志 | `/usr/local/edgion/logs/edgion-gateway.<YYYY-MM-DD>` | 配置同步、路由注册、启动/关闭 |
| controller 系统日志 | `/usr/local/edgion/logs/edgion-controller.<YYYY-MM-DD>` | 资源处理、sv 生成、gRPC 同步 |
| access log | `/usr/local/edgion/logs/edgion_access.log` | 每条请求一条 JSON，含 sv、stage_logs、upstream |
| TLS access log | `/usr/local/edgion/logs/tls_access.log` | TLS 握手/路由匹配结果 |

用 `orbstack-dev.sh` 的新命令读取（见下文），无需手写 `kubectl exec`。

---

## rv / sv / access log 三元组

三者是跨组件追踪的链路：

```
K8s 资源变更
  │  rv（resource_version，来自 metadata.resource_version）
  ▼
Controller process_resource()
  │  sv（sync_version，controller 生成，写入 annotation + gRPC 事件）
  ▼
Gateway 路由表更新
  │  sv（写入 MatchInfo，随每条请求打入 access log）
  ▼
access log entry  →  match_info.sv = <sv>
```

**用法**：
- 某条请求出问题 → 从 access log 找到 `match_info.sv` 值
- 用 `find-sv <sv>` 在 controller log 里找到对应的资源处理事件，看当时同步了什么配置
- 再用 `rv` 值在 controller log 里找到原始 K8s 资源变更

---

## 快速排查流程

### 第一步：确认 K8s 资源状态

```bash
# Pod 是否 Ready，GatewayClass/Gateway 是否 Accepted
./scripts/orbstack-dev.sh status

# 路由资源是否 Accepted / ResolvedRefs=True
./scripts/orbstack-dev.sh route-status grpc   # 只看 GRPCRoute
./scripts/orbstack-dev.sh route-status all    # 全部
```

如果路由 `Accepted=False` 或 `ResolvedRefs=False`，问题在控制面，不需要看数据面日志。

### 第二步：看 gateway 系统日志

```bash
# 最近 80 行（配置同步是否有 error）
./scripts/orbstack-dev.sh file-log g --tail 80

# 过滤关键词
./scripts/orbstack-dev.sh file-log g --grep "error\|warn\|No matching\|tls_route"
```

### 第三步：看 access log

```bash
# 最近 30 条请求（JSON 格式）
./scripts/orbstack-dev.sh access-log --tail 30

# 按 host 过滤
./scripts/orbstack-dev.sh access-log --grep "foo.com"

# 按路径过滤
./scripts/orbstack-dev.sh access-log --grep "/detect-misdirected"
```

access log 关键字段：
- `status` — HTTP 状态码（200/421/404 等）；gRPC 请求 status=200 但看 `grpc_status` 字段
- `match_info.sv` — 关联的 sync_version
- `stage_logs` — 各插件执行结果，格式 `OK u=alice; ` 或 `ERR ...`
- `upstreams` — 上游地址、延迟、重试次数
- `sni` — TLS SNI（调试 421 问题时关键）

### 第四步：用 sv 追踪配置来源

```bash
# 从 access log 拿到 sv 值（如 sv=42），跨日志追踪
./scripts/orbstack-dev.sh find-sv 42
```

输出三段：gateway log 里匹配 sv=42 的条目、controller log 里匹配的条目、access log 里对应的请求记录。

### 第五步：TLS 相关失败看 tls_access.log

```bash
./scripts/orbstack-dev.sh tls-log --tail 30
./scripts/orbstack-dev.sh tls-log --grep "foo.com"
```

tls_access.log 字段：`sni_hostname`、`gateway_name`、`status`（`Connected` / `NoMatchingRoute`）。

### 一键快照（失败后直接 dump）

```bash
# 收集全部相关状态到 stdout，可重定向到文件
./scripts/orbstack-dev.sh dump-debug > /tmp/debug-$(date +%H%M%S).txt
```

包含：Pods + GatewayClass + 所有路由 conditions + gateway/controller 最近日志 + access log 最近记录。

---

## 各 Profile 常见失败模式

### GATEWAY-GRPC

**现象**：`Internal(13)` 而非预期状态码

| 根因 | 如何确认 | 日志关键词 |
|------|---------|-----------|
| hostname 不匹配但 gateway 直接关闭 stream，未发 trailers | access log: status=200 但 grpc_status=13；gateway log: 无 unimplemented 相关输出 | `send_grpc_trailers_only` |
| 路由根本未注册 | `route-status grpc` 看 Accepted 状态 | `No matching GRPCRoute` |
| upstream H2C 连接失败 | access log 的 `upstreams` 字段有错误 | `h2c\|h2 upstream\|connect error` |

**现象**：`Unimplemented(12)` 正确，但 weight 路由失败（期望 OK(0)）

先确认 GRPCRoute resource 的 `status.conditions` 是否正常，再看 gateway 日志里 weight-based backend 选择是否有错误。

### GATEWAY-HTTP

**现象**：应返回 421 但返回 200（MisdirectedRequests）

```bash
# 检查 listener 的 Programmed 状态
kubectl get gateway same-namespace-with-https-listener \
  -n gateway-conformance-infra \
  -o jsonpath='{.status.listeners}' | jq .

# 检查 access log 中 sni 字段是否正确填充
./scripts/orbstack-dev.sh access-log --grep "detect-misdirected"
# 看 sni 字段是否为 example.org，host 是否为 second-example.org
```

### GATEWAY-TLS（需 `--tls-only` 环境）

```bash
# TLS 路由匹配失败最直接的日志
./scripts/orbstack-dev.sh tls-log --grep "NoMatchingRoute\|foo.com"

# gateway 系统日志里的 TLS 路由模块
./scripts/orbstack-dev.sh file-log g --grep "tls_route_manager\|passthrough"
```

---

## 单测 + 日志联动工作流

在一个终端跑单测，另一个终端监听日志：

```bash
# 终端 1：跑单测
cd examples/gateway-api-conformance
GO_RUN_FILTER="TestConformance/GRPCRouteListenerHostnameMatching$" bash scripts/run.sh

# 终端 2：实时看 access log（测试跑完后再看也可以）
./scripts/orbstack-dev.sh access-log -f

# 或者：测试后 dump 快照
./scripts/orbstack-dev.sh dump-debug
```

---

## 参考

- rv/sv 三元组架构：`skills/03-coding/00-logging-and-tracing-ids.md`
- access log 字段规范：`skills/03-coding/observability/00-access-log.md`
- TLS 专项调试（含更多 grep 模式）：`skills/09-misc/debugging-tls-gateway.md`
- 本地集成测试调试流程：`skills/06-tracing/00-debugging.md`
- 工具脚本：`examples/gateway-api-conformance/scripts/orbstack-dev.sh`
