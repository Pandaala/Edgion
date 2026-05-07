# Edgion Integration Testing

用于这些任务：

- 运行或缩小本地集成测试范围
- 新增一个 `examples/test/` 用例
- 排查 “YAML 已加载，但 gateway 行为不对 / 测试结果不稳定 / 证书或后端行为不匹配”

先读本页拿到主 workflow；需要具体映射时再读：

- 套件与目录映射： [references/integration-suite-map.md](references/integration-suite-map.md)
- `test_server` 能力清单： [references/test-server-capabilities.md](references/test-server-capabilities.md)
- 保留现场后的排障步骤： [../06-tracing/00-debugging.md](../06-tracing/00-debugging.md)

## macOS 前置：lo0 别名

`HTTPRoute/Backend/LB*` 测试用 `127.0.0.2` / `127.0.0.3` / `127.0.0.4` 作为 LeastConn / RoundRobin / ConsistentHash 的多个独立 endpoint。Linux 默认把 `127/8` 整段路由到 `lo`，CI（ubuntu-latest）开箱即用；**macOS 默认 `lo0` 只 bind `127.0.0.1`**，必须手动给 `lo0` 加 alias，否则 connect 静默 timeout（test_server bind `0.0.0.0` 也救不了）。

`run_integration.sh` 在启动时会做 preflight 检测，缺别名时直接退出并打印 `sudo ifconfig lo0 alias …` 命令；按提示手跑一次即可。重启后 alias 会失效，需要重新加；要永久生效用 launchd plist（脚本提示里有）。

## 快速开始

```bash
# 全量本地集成测试
./examples/test/scripts/integration/run_integration.sh

# 只跑某个 resource family
./examples/test/scripts/integration/run_integration.sh -r HTTPRoute

# 只跑某个 item
./examples/test/scripts/integration/run_integration.sh -r TLSRoute -i MultiSNI

# 已经 build 过后，跳过 prepare
./examples/test/scripts/integration/run_integration.sh --no-prepare -r EdgionPlugins -i KeyAuth

# 跑完保留现场，方便手动排查
./examples/test/scripts/integration/run_integration.sh --no-prepare --keep-alive -r Gateway -i StreamPlugins
```

## 先记住真实目录

- `examples/test/conf/`：测试 YAML，按 resource/item 组织
- `examples/code/client/`：Rust 测试客户端与 suite 实现
- `examples/code/server/test_server.rs`：本地 HTTP/gRPC/WebSocket/TCP/UDP 测试后端
- `examples/code/validator/`：`resource_diff`、`config_load_validator`
- `examples/test/scripts/integration/run_integration.sh`：主入口
- `examples/test/scripts/utils/`：`prepare.sh`、`start_all_with_conf.sh`、`load_conf.sh`、`kill_all.sh`
- `examples/test/scripts/gen_certs/`：测试证书生成脚本
- `integration_testing/testing_<timestamp>/`：本次运行的日志、PID、生成 Secret、报告目录

不要再按旧版测试目录约定去找代码或证书脚本。当前仓库实际代码在 `examples/code/`，证书脚本在 `examples/test/scripts/gen_certs/`。

## 运行链路

`run_integration.sh` 的真实职责是：

1. 调 `prepare.sh` 构建 `edgion-controller`、`edgion-gateway`、`edgion-cli`、`test_server`、`test_client`、`test_client_direct`、`resource_diff`、`config_load_validator`
2. 调 `start_all_with_conf.sh` 清现场、建 `integration_testing/testing_<timestamp>/`
3. 在 `${WORK_DIR}/generated-secrets/` 生成运行时 Secret，并在 suite YAML 之后再加载一次
4. 拉起 `test_server`、controller、gateway，并等待健康检查
5. 先加载 `examples/test/conf/base/`，再加载目标 suite 对应配置，最后加载运行时生成 Secret
6. 用 `resource_diff` 验证 controller 与 gateway 同步状态
7. 调 `test_client -g -r <Resource> -i <Item>` 执行测试
8. 默认清理；带 `--keep-alive` 时保留进程、日志和工作目录

几个关键事实：

- `--full-test` 会额外包含慢测试，并要求 Docker 环境可用
- `run_integration.sh` 会先检查 `ulimit -n`，不足时尝试提升到 `65535`
- `start_all_with_conf.sh` 会导出 `EDGION_WORK_DIR` 和 `EDGION_GENERATED_SECRET_DIR`
- `load_conf.sh` 会跳过模板 Secret，最终以工作目录里生成的版本覆盖

## 先分类，再写测试

`test_client` 现在的本地集成测试主要分成这些家族：

- `HTTPRoute`：Basic、Match、Backend、Filters、Protocol
- `GRPCRoute`：Basic、Match
- `Gateway`：Security、RealIP、AllowedRoutes、TLS、DynamicTest、StreamPlugins、PortConflict、Combined
- `TCPRoute` / `TLSRoute` / `UDPRoute`
- `EdgionPlugins`
- `EdgionTls`
- `ReferenceGrant` 状态测试
- `Services`（如 ACME）以及 `LinkSys` 的独立链路

对应的 YAML 目录、Rust suite、是否必须带 `--gateway`，统一见 [references/integration-suite-map.md](references/integration-suite-map.md)。

## 新增或修改一个本地集成测试

### 1. 先选最近的已有 suite

先在这些地方找最接近的模板：

```bash
rg -n "MultiSNI|KeyAuth|HealthCheckTransition|ReferenceGrant" examples/code examples/test/conf
```

优先复制“同协议、同验证方式、同依赖类型”的 case，不要从空白开始写。

### 2. 先改 YAML，再改 Rust

通常顺序是：

1. 在 `examples/test/conf/<Resource>/<Item>/` 写或改 YAML
2. 如需新 listener 端口，更新 `examples/test/conf/ports.json`
3. 如 YAML 用到新字段，先确认对应 CRD 已覆盖
4. 再去改 `examples/code/client/suites/...`

测试失败时，先确认是“配置没生效”还是“断言写错了”，不要一上来改 runtime 代码。

### 3. 复用 `test_server`，不要随手新造后端

大多数 HTTP、gRPC、WebSocket、TCP、UDP、ForwardAuth、OIDC、mirror、delay、status-code 场景，`test_server` 已经够用。只有当现有端点或协议确实不覆盖你的场景时，才扩 `examples/code/server/test_server.rs`。

`test_server` 已支持的后端端口和端点见 [references/test-server-capabilities.md](references/test-server-capabilities.md)。

### 4. 注册 suite 的触点要补全

新增 suite 时通常至少要同步这些位置：

- `examples/code/client/suites/<family>/...`
- `examples/code/client/suites/<family>/mod.rs`
- `examples/code/client/suites/mod.rs`
- `examples/code/client/test_client.rs`

如果只是往已有 family 下加 item，最容易漏的是 `test_client.rs` 里的：

- `resolve_suite()`
- `suite_to_port_key()`
- `add_suites_for_suite()`

### 5. 判断验证方式

优先顺序通常是：

1. 直接响应断言：状态码、header、body、握手是否成功
2. Access log：插件执行链、条件命中、内部字段、stage logs
3. Metrics：负载均衡、hash、一致性、延迟统计（**仅限已有 counter/gauge，不为插件单独新增**）
4. `resource_diff` / `config_load_validator`：配置是否被 controller/gateway 正确接受

经验上：

- 插件链路、条件执行、header/body 改写，用 access log 更稳
- LB、retry、hash、一致性，用 metrics 更稳
- `TLSRoute` / `EdgionTls` 常常需要同时看握手结果、gateway 日志和 access log

> **Metrics 断言范围限定**：metrics 断言只用于已有的 counter/gauge（如 `edgion_backend_requests_total`、`edgion_conn_ip_restriction_events_total`），**不应为验证插件行为而新增插件专属 counter**（如 `edgion_rate_limit_rejected_total`）。认证、限流、改写等插件的正确性验证走响应断言 + access log，已足够。原则见 [`03-coding/observability/01-metrics.md`](../03-coding/observability/01-metrics.md)「禁止事项：每个插件单独注册自己的 metrics」。

### 6. 缩小范围跑

先跑最窄命令：

```bash
./examples/test/scripts/integration/run_integration.sh --no-prepare -r <Resource> -i <Item>
```

如果需要直接重复打流量或自己控制 phase，再在保留现场后单独调用：

```bash
./target/debug/examples/test_client -g -r <Resource> -i <Item>
```

## 动态（热更新）测试

热更新测试由 Rust 侧 scenario 框架统一编排，作为完整集成测试的最后一步**默认自动执行**。

### 触发与查看

```bash
# 默认跑：含 dynamic scenarios（所有 family + 最后跑 dynamic）
./examples/test/scripts/integration/run_integration.sh

# 单独跑 -r Gateway：仍包含 dynamic（迁移自旧 run_dynamic_tests 行为）
./examples/test/scripts/integration/run_integration.sh -r Gateway

# 窄域跑（-r 指定非 Gateway，e.g. -r EdgionPlugins）：跳过 dynamic，加快迭代

# 列出已注册的 scenario id
./target/debug/examples/test_client --list-dynamic-scenarios

# 单独跑一个 scenario（保留现场后用）
./target/debug/examples/test_client -g --dynamic-scenario BasicAuthDynamic
```

完整 `run_integration.sh` 流程的最后一步会枚举 `--list-dynamic-scenarios` 的输出，逐个调用 `--dynamic-scenario`，每条以 `Dyn_<id>` 名义出现在 `report.log`。

### 当前 scenario 集合

| Scenario id | conf 目录 | 验证内容 |
|-------------|----------|---------|
| `GatewayDynamic` | `Gateway/DynamicTest/` | Gateway/HTTPRoute 配置变更（既有用例迁移） |
| `DslDynamic` | `EdgionPlugins/DslDynamic/DynamicTest/` | DSL 插件 source 切换 v1→v2 |
| `BasicAuthDynamic` | `EdgionPlugins/BasicAuth/DynamicTest/` | `secretRefs` 列表收缩，bob 凭据移除后 401 |
| `RateLimitDynamic` | `EdgionPlugins/RateLimit/DynamicTest/` | `onMissingKey` 从 Allow 切到 Deny |

### 框架在哪

- 框架代码：`examples/code/client/dynamic/`
  - `ctx.rs` — `ScenarioCtx`：组合算子 `assert_phase`、`wait_phase`、`apply_dir`、`delete_resources`、`sleep`（旧的 `wait_resource_sync` 方法已移除；底层 `wait::wait_for_resource_sync` 函数与 `examples/code/validator/resource_diff` binary 仍保留作为 setup-time sanity check）
  - `scenario.rs` — `Scenario` 结构 + `PhaseSuite` phase→suite factory 映射
  - `registry.rs` — 显式 `all()` 列表（新增 scenario 加一行即可）
  - `scenarios/<id>.rs` — 每个 scenario 一个文件，N-phase 状态机用 `ScenarioCtx` 串起来
- 设计文档：`docs/superpowers/specs/2026-04-25-plugin-dynamic-test-framework-design.md`
- 实施计划：`docs/superpowers/plans/2026-04-25-plugin-dynamic-test-framework.md`

### 新增一个动态 scenario

1. 在 `examples/test/conf/<Resource>/<Item>/DynamicTest/` 写 `initial/` + `updates/` YAML（`load_conf.sh` 自动跳过 `updates/` 与 `delete/`）
2. 在 `examples/code/client/suites/<family>/dynamic.rs` 写两个 phase suite（`<Name>InitialTestSuite` / `<Name>UpdateTestSuite`），通过 family `mod.rs` re-export 到 `suites::*`
3. 在 `test_client.rs` 三处 dispatch 表加分支：`resolve_suite`（如需 alias）、`suite_to_port_key`、`add_suites_for_suite`（按 `--phase` 选择 suite）
4. 在 `examples/code/client/dynamic/scenarios/<id>.rs` 写 scenario，`run` 函数按 N-phase 顺序串：`assert_phase`（初态） → `apply_dir` / `delete_resources`（变更） → `wait_phase`（带 timeout，等终态业务断言全 pass）。**不要**用 `sleep` 兜底，也不要用 `wait_resource_sync`——前者不可靠，后者已不在 ScenarioCtx 上暴露
5. 在 `dynamic/scenarios/mod.rs` 加 `pub mod <id>;`
6. 在 `dynamic/registry.rs` 的 `all()` 追加 `crate::dynamic::scenarios::<id>::SCENARIO`

之后 `--list-dynamic-scenarios` 会自动列出，`run_integration.sh`（默认）会在完整跑或 `-r Gateway` 路径下自动包含。

### 手动按 phase 调试

scenario 内部用 `ctx.assert_phase(scenario, "initial"|"update")` 调度 suite。手动需要绕开编排时仍可用旧路径（保留现场后）：

```bash
# 只跑 initial 阶段断言
./target/debug/examples/test_client -g -r <Resource> -i <Item> --phase initial
# 手动 apply updates/ 后再跑 update 阶段断言
./target/debug/edgion-cli --server http://127.0.0.1:5800 \
    apply -f examples/test/conf/<Resource>/<Item>/DynamicTest/updates/
sleep 3
./target/debug/examples/test_client -g -r <Resource> -i <Item> --phase update
```

`--phase` 参数仍然有效（每个 dynamic dispatch 都按 phase 路由到对应 suite），是排查 scenario 内部某 phase 行为时的常用入口。

> 注：scenario 内部不再用固定 `sleep` 等同步——见上面 §动态测试 ScenarioCtx 算子列表的 `wait_phase`。手动调试场景仍可用 `sleep`，因为人在终端可以看 log 决策超时。

## 常见排查顺序

1. 先看 `${WORK_DIR}/report.log` 和 `${WORK_DIR}/test_logs/<case>.log`
2. 再看 `${WORK_DIR}/logs/controller.log`、`${WORK_DIR}/logs/gateway.log`
3. 涉及插件或路由命中时，再看 access log / `tls_access.log`
4. 怀疑配置未加载时，用 `edgion-cli` 或 admin API 查 controller/gateway 当前资源
5. 怀疑测试后端行为不符时，再回到 `test_server` 端口和端点能力核对

详细命令和日志入口见 [../06-tracing/00-debugging.md](../06-tracing/00-debugging.md)。

## 认证在集成测试中的行为

集成测试中认证由框架自动处理，无需手动操作：

- Controller 启动时自动生成随机凭据，写入 `{WORK_DIR}/config/resources/Secret_edgion-system_edgion-admin-credentials.yaml`
- `load_conf.sh` 调用 `edgion-cli` 时，`ensure_auth()` 自动通过 `EDGION_WORK_DIR` 找到配置 → 读取 Secret → 完成登录
- `start_all_with_conf.sh` 导出 `EDGION_WORK_DIR`，使自动认证路径可达
- CI/测试环境无需任何手动 `edgion-cli login` 步骤

### Center 无认证配置回归测试

`examples/test/scripts/integration/test_center_no_auth.sh` 是一个独立自包含的集成层回归，
验证 Center 在无 `[local_auth]` + 无 `[auth]` 时：server 启动成功、`/health` 与
`/api/v1/auth/status` 返回 200、`/api/v1/controllers` 等业务路径返回 503（fail-close）、
启动日志包含 `No authentication configured` WARN。作为 fix #01 bug 1/4 + Task 4 fix 的
结构性回归被纳入 `run_integration.sh` 的前置检查，不依赖 Controller/Gateway/test_server 环境。

## Gateway / Controller 复用原则

> 集成测试的设计前提：**同一类测试复用同一套 Gateway + Controller**。新增 suite 默认挂主 `run_integration.sh` 的进程，**不得**自己 spawn 新的 `edgion-gateway` / `edgion-controller`。

### 主流程

`run_integration.sh` → `start_all_with_conf.sh` 启动 1 套 Gateway + 1 套 Controller，所有 family 的 suite（HTTPRoute / GRPCRoute / TCPRoute / UDPRoute / TLSRoute / EdgionPlugins / EdgionTls / Gateway / Status / LinkSys / dynamic scenarios）共享同一进程，通过 `TestContext::apply_yaml()` / `delete_resource()` 在线变更配置驱动 Controller 同步到 Gateway，无重启。

### 独立拓扑例外（合理）

只有需要**完全不同的进程级配置**才独立启动，每个独立场景仍只启一套：

| 脚本 | 独立拓扑原因 |
|---|---|
| `test_file_watcher.sh` | Controller FileSystem 源（与 K8s 模式互斥） |
| `test_resource_abnormal_metrics.sh` | Controller `tick_secs=30` 慢测试专用 |
| `test_center_no_auth.sh` | Center 无 auth fail-close 验证（无 Controller/Gateway） |
| `run_center_test.sh` | 1 Center + 2 Controllers 联邦验证 |
| `run_fed_sync_test.sh` | Center + 多 Controllers 联邦同步 |
| `run_conf_sync_mtls_test.sh` | Controller↔Gateway mTLS 通道 |
| `run_acme_test.sh` | 独立 ACME 拓扑 |

新增独立脚本必须在脚本头部注释里说明"为什么不能复用主环境"，否则 PR 不予合并。

### 反模式（reviewer 阻塞合并的信号）

新 suite / 新脚本里出现以下任意一项，需要 reviewer 阻塞合并并要求作者改为复用主进程或并入上表例外：

- `Command::new("edgion-gateway")` / `Command::new("./edgion-gateway")` / `spawn_gateway` 类调用
- 在 `examples/code/client/` 或 `examples/test/scripts/` 新加非例外清单内的脚本调用 `start_all` / `start_all_with_conf`
- 同一脚本内反复 kill / 重启 `edgion-gateway`（除非测的就是 Gateway 重启行为本身——此时也只允许 1 次重启循环）

合理的 `Command::new` 例子（不阻塞）：调用外部工具如 `openssl`、`docker`、`curl` 验证客户端协议；它们不属于"启 Gateway"。

### CI 兜底

`run_integration.sh` 启动后会做 `pgrep -c edgion-gateway` 自检，运行期间检测到 ≥ 2 个 `edgion-gateway` 进程则 fail fast 并退出（避免悄悄违规）。如果你的新 suite 触发了这个检查，先看是不是漏了 cleanup 或误用了 `Command::new("edgion-gateway")`。

## 容易踩坑的地方

- YAML 在 `examples/test/conf/` 写了，但 `test_client.rs` 没注册，结果用例根本没跑到
- 新增 item 忘了补 `suite_to_port_key()`，流量打到了错误 listener
- `--gateway` 漏掉，导致本应走 gateway 的 suite 直接报错
- 复用了旧测试证书，导致 SAN 不匹配
- 运行时生成 Secret 在 `${WORK_DIR}/generated-secrets/`，却只盯着模板 YAML 看
- 以为是 gateway bug，实际是 `test_server` 路径或端口不匹配
- 直接改 `LinkSys` 或 K8s 场景，却没有切到对应 skill 文档

## 什么时候继续读别的文档

- 要看 suite/resource/item 的具体映射： [references/integration-suite-map.md](references/integration-suite-map.md)
- 要确认 `test_server` 端口、端点、OIDC/auth、mirror 能力： [references/test-server-capabilities.md](references/test-server-capabilities.md)
- 要保留现场做手工排查： [../06-tracing/00-debugging.md](../06-tracing/00-debugging.md)
- 要跑 K8s 版本： [02-k8s-integration-testing.md](02-k8s-integration-testing.md)
- 要跑 LinkSys： [03-link-sys-testing.md](03-link-sys-testing.md)
