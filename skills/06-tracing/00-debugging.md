---
name: debugging-and-troubleshooting
description: Day-to-day development debugging guide for Edgion. Use when local code changes break routing, plugins, sync, readiness, or runtime behavior and you need a fast workflow using keep-alive integration runs, Admin API, edgion-ctl, access-log store, metrics, and resource_diff.
---
# 调试与排障

> 面向日常开发排障，不是“怎么写测试”，而是“代码改完后现在到底坏在哪一层”。
> 如果问题明显集中在 TLS Gateway，请优先补读 [../09-misc/debugging-tls-gateway.md](../09-misc/debugging-tls-gateway.md)。

## 先选调试入口

### 1. 配置 / 同步链路问题

症状：
- `edgion-ctl apply` 成功，但 Gateway 没生效
- Controller `ready` 一直不通过
- Gateway `ready` 一直不通过
- 改了 YAML，但 `server` / `client` 视图不一致

优先使用：
- `edgion-ctl --target center/server/client`
- `examples/resource_diff`
- Controller/Gateway Admin API

### 2. 路由 / 插件运行时问题

症状：
- 返回 404 / 421 / 502 / 503
- 插件没执行、顺序不对、条件没命中
- 路由命中了错误的 Gateway / Listener / Backend

优先使用：
- Access Log Store
- Gateway Admin API
- Gateway 日志和 `store-stats`

### 3. 负载均衡 / 统计行为问题

症状：
- 流量分布不均
- 一致性哈希不稳定
- 重试次数和预期不符

优先使用：
- `http://127.0.0.1:5901/metrics`
- `MetricsClient`
- `resource_diff`

## 最快的本地调试方式

### 保留现场跑单个用例

```bash
./examples/test/scripts/integration/run_integration.sh --keep-alive -r <Resource> -i <Item>
```

适合：
- 先用仓库现成测试配置把现场拉起来
- 测完后保留 controller / gateway / test_server 进程和日志目录
- 再手工 `curl`、`edgion-ctl`、查日志

### 跳过编译快速迭代

```bash
./examples/test/scripts/integration/run_integration.sh --no-prepare --keep-alive -r <Resource> -i <Item>
```

### 手工启动完整测试环境

```bash
./examples/test/scripts/utils/start_all_with_conf.sh
```

这会：
- 创建 `integration_testing/testing_YYYYMMDD_HHMMSS/`
- 导出 `EDGION_WORK_DIR`
- 启动 `test_server`
- 启动 controller
- 载入 base config 与 suite config
- 启动 gateway
- 最后执行一次 `resource_diff`

## 第一步：看服务是否真的活着

### Controller

```bash
curl -sf http://127.0.0.1:5800/health
curl -sf http://127.0.0.1:5800/ready
curl -sf http://127.0.0.1:5800/api/v1/server-info
```

### Gateway

```bash
curl -sf http://127.0.0.1:5900/health
curl -sf http://127.0.0.1:5900/ready
curl -sf http://127.0.0.1:5900/api/v1/server-info
curl -sf http://127.0.0.1:5901/health
```

当前实现里要特别记住：
- Gateway Admin API 端口当前固定是 `5900`
- Gateway Metrics API 端口当前固定是 `5901`
- `gateway.admin_listen` 字段目前存在于配置结构里，但启动逻辑还没有真正用它

## 第二步：用 center / server / client 三视图定位问题层次

`edgion-ctl` 的三种 target 非常适合快速定位：

```bash
./target/debug/edgion-ctl get httproute -n default
./target/debug/edgion-ctl -t server get httproute -n default
./target/debug/edgion-ctl -t client get httproute -n default
```

判断规则：

| 现象 | 大概率问题层 |
|------|--------------|
| `center` 没有资源 | 配置没真正 apply 进去，或 kind / namespace / name 用错 |
| `center` 有，`server` 没有 | Controller `ProcessorHandler` 校验 / preparse / parse / requeue 链路 |
| `server` 有，`client` 没有 | gRPC sync、`no_sync_kinds`、Gateway readiness、watch/list 过程 |
| `client` 有，但流量行为不对 | Gateway `ConfHandler`、运行时 store、route/plugin/tls/backend 逻辑 |

如果是批量核对，直接跑：

```bash
./target/debug/examples/resource_diff
```

或指定地址：

```bash
./target/debug/examples/resource_diff \
  --controller-url http://127.0.0.1:5800 \
  --gateway-url http://127.0.0.1:5900
```

## 第三步：看关键 Admin API

### Controller 侧

- `/api/v1/server-info`
- `/api/v1/reload`
- `/api/v1/namespaced/{kind}`
- `/api/v1/cluster/{kind}`
- `/configserver/{kind}/list`
- `/configserver/{kind}?name=...&namespace=...`

示例：

```bash
curl -s http://127.0.0.1:5800/configserver/httproute/list | jq .
curl -s "http://127.0.0.1:5800/configserver/httproute?namespace=default&name=my-route" | jq .
```

### Gateway 侧

- `/api/v1/server-info`
- `/configclient/{kind}/list`
- `/configclient/{kind}?name=...&namespace=...`
- `/api/v1/debug/store-stats`

示例：

```bash
curl -s http://127.0.0.1:5900/configclient/httproute/list | jq .
curl -s http://127.0.0.1:5900/api/v1/debug/store-stats | jq .
```

`store-stats` 特别适合看：
- route manager 里到底有没有对象
- TLS store / plugin store / backend policy store 有没有泄漏或残留

## 第四步：抓 Access Log Store，看插件和路由实际执行了什么

前提：Gateway 必须带 `--integration-testing-mode` 启动。

### 直接用 HTTP 请求抓

1. 请求时带：
   - `x-trace-id: <唯一值>`
   - `access_log: test_store`
2. 再去查：

```bash
curl -s http://127.0.0.1:5900/api/v1/testing/status | jq .
curl -s http://127.0.0.1:5900/api/v1/testing/access-log/<trace-id> | jq .
```

### 你应该重点看这些字段

- `request_info.host`
- `request_info.path`
- `matched_route`
- `backend`
- `stage_logs`
- `errors`

如果是插件问题，`stage_logs` 最有用；如果是上游问题，优先看 `backend.upstreams` 和 `errors`。

代码侧现成客户端在：
- `examples/code/client/access_log_client.rs`

## 第五步：看 Metrics

Metrics 服务在：

```bash
curl -s http://127.0.0.1:5901/metrics
```

适合验证：
- 负载均衡分布
- 一致性哈希
- 重试与上游请求次数
- 日志丢弃计数

如果问题是“统计行为不符合预期”，优先走 metrics，不要只看单次响应。

## 第六步：看日志目录和关键文件

集成测试现场最常用的是：

- `${EDGION_WORK_DIR}/logs/gateway.log`
- `${EDGION_WORK_DIR}/logs/controller.log`
- `${EDGION_WORK_DIR}/logs/edgion_access.log`
- `${EDGION_WORK_DIR}/logs/ssl.log`
- `${EDGION_WORK_DIR}/logs/tls_access.log`

如果是 `run_integration.sh` 生成的现场，优先看：

- `integration_testing/<timestamp>/logs/`
- `integration_testing/<timestamp>/test_logs/`
- `integration_testing/<timestamp>/report.log`

## 常见症状速查

| 症状 | 先看哪里 |
|------|---------|
| `edgion-ctl apply` 成功但 Gateway 没变化 | 三视图对比 + `resource_diff` |
| Controller `ready` 不通过 | `gatewayclass` / `gateway` / `processor` 日志，必要时看 `/api/v1/server-info` |
| Gateway `ready` 不通过 | `client` 侧缓存、`supported_kinds`、`server_id`、watch/list 日志 |
| 404 / no match | `configclient` 资源、`store-stats`、Access Log `matched_route` |
| 502 / 503 | `Service` / `EndpointSlice` / `Endpoint`、backend store、metrics、上游日志 |
| 插件没执行 | Access Log Store 的 `stage_logs` |
| TLS 握手或证书问题 | `ssl.log`、`tls_access.log`、[../09-misc/debugging-tls-gateway.md](../09-misc/debugging-tls-gateway.md) |

## 如果是配置层面的问题

很多“为什么改了 TOML / work_dir / conf_dir 没生效”的问题，本质上不是调试，而是配置层次没选对。
这类问题优先转去：

- [../02-features/02-config/SKILL.md](../02-features/02-config/SKILL.md)

## 代码入口速查

- Controller Admin API：`src/core/controller/api/`
- Gateway Admin API：`src/core/gateway/api/mod.rs`
- `edgion-ctl`：`src/core/ctl/cli/`
- Gateway 启动入口：`src/core/gateway/cli/mod.rs`
- Controller 启动入口：`src/core/controller/cli/mod.rs`
- 资源同步核对工具：`examples/code/validator/resource_diff.rs`
