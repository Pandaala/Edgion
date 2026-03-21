# LinkSys Development Guide

用于这些任务：

- 新增或扩展 `LinkSys` 外部系统连接能力
- 调整 Redis / Etcd / Elasticsearch / Webhook 的运行时接线
- 排查 “LinkSys YAML 已同步，但 gateway 侧 runtime client / webhook manager 没更新”

先读这些真实入口：

- [../../src/types/resources/link_sys/mod.rs](../../src/types/resources/link_sys/mod.rs)
- [../../src/core/controller/conf_mgr/sync_runtime/resource_processor/handlers/link_sys.rs](../../src/core/controller/conf_mgr/sync_runtime/resource_processor/handlers/link_sys.rs)
- [../../src/core/gateway/conf_sync/conf_client/config_client.rs](../../src/core/gateway/conf_sync/conf_client/config_client.rs)
- [../../src/core/gateway/link_sys/runtime/conf_handler.rs](../../src/core/gateway/link_sys/runtime/conf_handler.rs)
- [../../src/core/gateway/link_sys/runtime/store.rs](../../src/core/gateway/link_sys/runtime/store.rs)
- [../../src/core/gateway/link_sys/providers/webhook/mod.rs](../../src/core/gateway/link_sys/providers/webhook/mod.rs)
- [../04-testing/02-link-sys-testing.md](../04-testing/02-link-sys-testing.md)

## 先分清你改的是哪一类东西

LinkSys 现在不是“每种外部系统一个资源 kind”，而是一个统一的 `LinkSys` CRD，加 `SystemConfig` 变体：

- `Redis`
- `Etcd`
- `Elasticsearch`
- `Webhook`
- `Kafka` 目前只有 schema placeholder，还没有完整 runtime 落地

另外还有一个容易混淆的点：

- `local_file` provider 在 `src/core/gateway/link_sys/providers/local_file/`
- 但它不是 `LinkSys` 的 `SystemConfig` 变体，而是日志落盘用的本地 `DataSender`

所以先判断：

1. 你是在给已有 `LinkSys` 增加一个新的 `SystemConfig` 变体？
2. 你是在扩已有 provider 的 runtime client / ops / config mapping？
3. 你其实只需要一个非 CRD 的内部 provider（类似 `local_file`）？

大多数 LinkSys 开发任务，答案其实是 1 或 2，不是“加一个新资源 kind”。

## 当前真实心智模型

当前主链路是：

1. `LinkSys` YAML 进入 controller
2. `impl_resource_meta!(LinkSys, ...)` 和 gateway 侧 `ConfHandler` 都会调 `validate_config()`
3. controller `LinkSysHandler` 主要负责标准 status 更新，不做复杂依赖解析
4. gateway `ConfigClient` 里有专门的 `ClientCache<LinkSys>`
5. 这个 cache 注册了 `create_link_sys_handler()`
6. `LinkSysStore.full_set()` / `partial_update()` 会把资源分发到各个 runtime manager
7. 调用方最终通过 typed API 取 runtime client，例如：
   - `get_redis_client("namespace/name")`
   - `get_etcd_client("namespace/name")`
   - `get_es_client("namespace/name")`
   - `get_webhook_manager().get("namespace/name")`

也就是说：

- controller 侧主要管“资源是否合法、状态是否正确”
- gateway 侧 `LinkSysStore` 才是 runtime 生命周期入口

## 当前已落地的子系统

### 1. Redis / Etcd / Elasticsearch

这三类都走“CRD 配置 -> runtime client -> typed store / ops”的模式。

通常目录长这样：

- `src/types/resources/link_sys/<system>.rs`
- `src/core/gateway/link_sys/providers/<system>/client.rs`
- `src/core/gateway/link_sys/providers/<system>/config_mapping.rs`
- `src/core/gateway/link_sys/providers/<system>/ops.rs`

其中：

- `client.rs` 管生命周期和底层客户端包装
- `config_mapping.rs` 负责 CRD 配置到库配置的转换
- `ops.rs` 暴露 Edgion 真正需要的高层操作

`Elasticsearch` 还额外有：

- `bulk.rs`
- `data_sender.rs`

因为它现在也承担日志等 `DataSender<String>` 场景。

### 2. Webhook

Webhook 不是普通 KV client，而是一个独立的 webhook service manager 模式：

- `providers/webhook/manager.rs`：全局注册表
- `providers/webhook/runtime.rs`：运行时状态
- `providers/webhook/health.rs`：主动健康检查
- `providers/webhook/resolver.rs`：给 `KeyGet::Webhook` 这类路径消费

关键点：

- key 仍然是 `namespace/name`
- `WebhookManager.upsert()` 会按配置重建 HTTP client 和健康检查任务
- 删除资源时要正确 abort 旧 health task

### 3. Kafka

当前 `SystemConfig::Kafka` 只有 placeholder schema。不要把它当成已经有 provider/runtime/store 接线的完整实现。

## 真实的 ConfHandler 桥接模式

LinkSys 现在的桥接关系已经固定了：

1. `ConfigClient::new()` 里创建 `ClientCache<LinkSys>`
2. 调 `create_link_sys_handler()` 注册 conf processor
3. `create_link_sys_handler()` 返回全局 `LinkSysStore`
4. `ConfHandler<LinkSys>::full_set()` / `partial_update()` 先做 `validate_config()`
5. 再调用 `LinkSysStore.replace_all()` / `update()`
6. `runtime/store.rs` 里的 `dispatch_full_set()` / `dispatch_partial_update()` 异步分发到 Redis / Etcd / ES / Webhook runtime

因此，如果你改的是 gateway 侧 runtime 行为，重点看的是：

- [conf_handler.rs](../../src/core/gateway/link_sys/runtime/conf_handler.rs)
- [store.rs](../../src/core/gateway/link_sys/runtime/store.rs)

不是再自己造一套旁路 watcher。

## 新增一个 LinkSys 系统，通常要改哪些地方

如果你是在现有 `LinkSys` 资源里新增一个 system variant，通常至少要碰这些位置：

1. `src/types/resources/link_sys/<system>.rs`
2. `src/types/resources/link_sys/mod.rs`
3. `config/crd/edgion-crd/link_sys_crd.yaml`
4. `src/core/gateway/link_sys/providers/<system>/`
5. `src/core/gateway/link_sys/runtime/store.rs`
6. 必要时 `src/core/gateway/api/mod.rs`
7. `examples/test/conf/LinkSys/<System>/`
8. `examples/test/conf/Services/<system>/docker-compose.yaml`
9. `examples/test/scripts/integration/run_<system>_test.sh`

通常不需要新增新的 resource kind，也不需要额外改 controller processor 框架本身。

## 推荐开发顺序

### 1. 先补类型和 schema

先定义或扩展：

- `SystemConfig` 变体
- `<System>ClientConfig`
- `validate_config()` 分支
- CRD schema

如果 schema 还没补齐，就不要先写人类文档或测试 YAML。

### 2. 再补 provider runtime

优先把 provider 目录拆成清晰的三层：

- config mapping
- runtime client
- high-level ops

不要把 CRD 类型、库配置转换、业务操作全塞进一个文件。

### 3. 再接入 `runtime/store.rs`

这里是最容易漏的地方。至少要补：

- `dispatch_full_set()` 分支
- `dispatch_partial_update()` 分支
- typed runtime store 的 insert / remove / replace-all 逻辑
- 删除时旧 client / 旧任务的清理逻辑

如果系统支持热更新，优先复用“先 swap 新实例，再后台 init 或 shutdown 旧实例”的已有模式。

### 4. 只在测试需要时补 admin testing endpoints

如果 integration test 需要直接验证客户端行为，就在：

- `src/core/gateway/api/mod.rs`

里补 testing router 的 endpoint。

当前仓库已经有这几类 testing endpoints：

- Redis
- Etcd
- Elasticsearch

它们只在 `--integration-testing-mode` 下挂到 gateway admin API。

### 5. 最后补测试与 compose

现有脚本是真实存在的：

- `examples/test/scripts/integration/run_redis_test.sh`
- `examples/test/scripts/integration/run_etcd_test.sh`
- `examples/test/scripts/integration/run_es_test.sh`

配置与依赖也已经分层：

- `examples/test/conf/LinkSys/<System>/`
- `examples/test/conf/Services/<system>/docker-compose.yaml`

新增系统时，优先照着这套结构补，不要自己再发明一套测试脚本命名。

## 什么时候不要走 LinkSys

这些场景通常不该硬塞进 `LinkSys`：

- 只是一个进程内工具型 sender，没有 CRD 配置需求
- 不需要按 `namespace/name` 动态更新
- 不需要 controller -> gateway 同步
- 只是日志本地落盘这类内部能力

这种情况更接近 `local_file` provider，而不是新 `SystemConfig`。

## 常见坑

- 只加了 `SystemConfig` 变体，忘了补 `validate_config()` 或 CRD
- provider 已经写完，但 `runtime/store.rs` 没分发，导致 YAML 已同步但 runtime 不生效
- 只补了 full sync，忘了 partial update / remove 路径
- Webhook 更新时没有正确清理旧 health task
- 把 LinkSys 当成“新 resource kind”，结果改了很多不该动的 controller 框架代码
- 写了测试 compose 和 YAML，却没补 gateway admin testing endpoint，导致很难断言运行时行为

## 验证

先做最小静态检查：

```bash
cargo check
```

如果改了某个现有系统，优先跑对应脚本：

```bash
./examples/test/scripts/integration/run_redis_test.sh
./examples/test/scripts/integration/run_etcd_test.sh
./examples/test/scripts/integration/run_es_test.sh
```

如果你改的是文档 / skill / AI 入口，也别忘了：

```bash
make check-agent-docs
```
