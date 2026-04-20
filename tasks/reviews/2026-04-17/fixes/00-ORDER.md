# Fixes 执行索引 & 进度跟踪

> **这是唯一的 index 文件。修完一个工单就**勾选 + 删 fix 文件**，两件事一起做才算闭环。**

## 使用规范（每修一个工单必走）

1. **开工前** — 在本文件找到工单编号，确认：
   - 左边 checkbox 是 `- [ ]`（未修）
   - **依赖**列为空，或所列依赖都已 `- [x]`
   - **同文件协同**列若有其他工单编号，考虑合并做（或等另一侧先做）
2. **修代码** — 打开 `NN-*.md` 按工单步骤改
3. **验证** — `cargo check --all-targets` + `cargo clippy --all-targets` + 相关测试
4. **闭环三件套**（**必须一起做**）：
   ```
   a. git commit 代码  # message 带 "fixes #NN"
   b. rm tasks/reviews/2026-04-17/fixes/NN-*.md
   c. 本文件对应行 [ ] → [x]
   ```
5. **提交闭环** — `git add tasks/reviews/2026-04-17/fixes/` + commit（可合进 a）

## 防重复检查（每次修文件前 10 秒查一次）

```bash
# 查某个代码文件还被哪些未修工单涉及（避免同文件反复改）
rg "<file_path_or_key>" /Users/caohao/ws2/Edgion/tasks/reviews/2026-04-17/fixes/
```

如果命中 2+ 个工单，**合并一次改**（commit message 写 `fixes #N1, #N2`）。

---

## Wave 1 — 零依赖小修复（快速清 10 个）

> 任意顺序可并行，推荐按从小到大。**可以一天清完。**

- [x] **#02** — Secret 请求体 tracing 泄漏（`controller/api/namespaced_handlers.rs:104`）— **XS** — 删一行
- [x] **#18** — Federation aggregator 一行修复（`center/fed_sync/server/mod.rs:148`）— **XS** — 已在 commit ad14749（GCIR 集成测试）中预先修复，本次仅档案闭环
- [x] **#25** — ServerCache.watchers 删除字段 — **S**
- [x] **#26** — JWKS body 大小限制（C 方案：`fetch_json_limited` helper = Content-Length 预检 + `bytes_stream()` 累加截断；覆盖 admin auth 2 处 + plugin OIDC 3 处；`AdminAuthConfig` + `OpenidConnectConfig` 各加可配置字段 64KB/1MB/128KB，向后兼容）— **S**
- [x] **#07** — ACME/CB/redirect tracing（3 处同类删除 + ACME lookup counter `edgion_acme_challenge_lookup_total{result}` 增补）— **S**
- [x] **#23** — RR u16 截断补 counter `edgion_rr_selector_truncated_total`（无 label，全局 `Once` 去重 warn）— **S**
- [x] **#17** — RateLimit PluginLog 不含 API key（格式收敛为 `rl:<verdict>,kl=<len>,...` ≤14B；同文件 `rate_limit_redis/plugin.rs` 已一起改，#13 再动此文件时行级无重叠）— **S**
- [x] **#08** — ES DataSender 状态机（`AtomicBool` CAS 去重 info，per-record warn 移除；构造期一次性警告 `failed_cache.is_none()`；counter `edgion_link_sys_es_send_total{result=ok|failed_fallback|failed_no_cache}`）— **S**
- [x] **#09** — GlobalConnectionIpRestriction tracing（3 处删除 + `edgion_conn_ip_restriction_events_total{result}` counter 增补）— **S**
- [x] **#15** — ExtensionRef 错误响应体（async Request stage 写 500 + JSON body；`PluginRunningResult::ErrTerminateRequest` 定义处加约定注释；sync `UpstreamResponseFilter` stage 因无 await 保留原样）— **S**

---

## Wave 2 — Federation 生命周期链（**必须按顺序**）

- [x] **#19** — Federation offline 资源泄漏 — 路线 Y 简化版（`mark_offline` 释放 `stream_tx` + session_id race 守卫 + 删 evict_stale/offline_evict_hours 死代码）。commits: `c37519d` + `1e7c79f` + `8e1a42d` + `e17f53a`。**M**
  - 依赖：— （Wave 1 的 #18 建议先做，同文件 `server/mod.rs`）
  - 同文件协同：`center/fed_sync/server/mod.rs`（#18/#20）、`registry/mod.rs`（#20）
- [x] **#20** — CenterDb 写路径 + Admin DELETE 级联 — 补齐 `upsert_controller` 调用（register + 3 个 offline 分支），新增 `CenterDb::mark_offline` narrow UPDATE（不覆盖 cluster/env/tag），Admin DELETE 5 路级联（registry+aggregator+watch_cache+metadata_store 经 handler 链间接+DB），`list_resources` 过滤 offline keys。commits: `f1b9316` + `be7804c` + `419561c`。**M**
  - 依赖：**#19**（需要 remove 方法）
  - 同文件协同：`server/mod.rs`（#18/#19 已完成）、`registry/mod.rs`（#19 已完成）、`api/mod.rs`（#01 并行安全）
- [x] **#24a** — Center/Controller `/metrics` 端点 infra（admin router 挂载 + auth skip + 共享 `common/observe/metrics_api`）— **S**
  - 依赖：无
  - 变更：新建 `src/core/common/observe/metrics_api.rs`；Gateway `observe/metrics/api.rs` 改薄壳 re-export；Controller/Center `run()` 早调 `install_global_recorder`；`unified_auth` skip_paths 加 `/metrics`
- [x] **#24b** — Federation metrics baseline（14 条，已在此分支落盘）— **M**
  - 依赖：**#19 + #20 + #24a**
  - 变更（相较原 ticket）：4 条 histogram → 2 条 `_seconds_last` gauge + 2 条 drop（违反项目"无 Histogram"硬规）；`controller_id` label 去掉换成 `watch_errors_total{kind,reason}` counter；`stream_tx_channel_size` 换成 `stream_tx_drops_total`，但 fed_sync 当前无 `try_send` 路径故本工单不落盘该条（共 14 条实际落盘）
  - 实现：`src/core/common/observe/fed_metrics.rs`（定义 + labels 子模块）+ 注入点在 `center/fed_sync/{registry,server}`、`center/aggregator`、`center/api/{consistency_handlers,global_connection_ip_restriction_handlers}`、`controller/fed_sync/fed_client`
  - 与 Wave 5 #21/#22/#23 的 Gateway metrics 整改独立

---

## Wave 3 — 认证 & 敏感数据（设计性改动）

- [x] **#14** — Plugin Entry 阶段错配 → 认证绕过 — **M**
  - 依赖：—
  - **为什么先做**：让插件错配在 status 中暴露，后续插件改动更安全
  - 同文件协同：`plugins/runtime/plugin_runtime.rs`（Wave 4 的 #13 也碰，**先 #14 后 #13**）
- [x] **#01** — Admin API 认证 fail-open 全链路 — **L** — commits `c013d88`/`ab5ace3`/`b07fb92`/`00b8ac1`/`3783247`/`6b02b77`/`e79bd5b`/`c7fc43b`/`15d2b33`/`2cb3546`/`4a1d637`/`fcf9947`/`4e36487`（Tasks 1–7 functional + 6 style）。`UnifiedAuthState` 引入 `require_auth`+`ArcSwapOption<LocalAuthState>`+`update_local`；`from_configs` validate 失败返 Err；middleware `require_auth && 两 provider 都 None` 返 503+Retry-After:10+ApiResponse JSON；新 `AuthStatus` 枚举（`Ready`/`Initializing`/`Disabled`）+ `active_providers: Vec<String>`（`AuthStatusResponse` 删 `auth_method`）；login/me/logout handler 改用共享 `Arc<UnifiedAuthState>`，login 预检 local 未就绪返 503；Controller startup 引入 `derive_require_auth`（intent-based）+ `resolve_local_config`（tolerant, returns needs_retry）；`secret_retry_loop` 指数退避 1→30s cap、10min 硬 timeout、`process::exit(1)`、`update_local` 成功回写；CLI `ensure_auth` 严格分类（404/5xx/TLS/JSON 全部 bail，`AuthStatus::{Disabled→Ok, Initializing→bail, Ready→continue}`，`active_providers` 选 provider）。Task 8 集成测试降级为 unit test 覆盖（`from_configs` validate-Err + `main()->Result<()>` 无需 shell scaffolding 验证退出码；已记入设计 doc §7.2 Path C）。
- [ ] **#16** — ctx_map 敏感数据 + denylist append — **L**（短期 S）
  - 依赖：—
  - 同文件协同：`ctx_set/plugin.rs:230` 也是 Wave 4 #13 的删除点，**#16 里一起删**
- [x] **#28** — CLI token 权限 + 密码擦除 — **S**
  - 依赖：—
- [x] **#27** — IpRestriction schema 迁移保护 — **M**
  - 依赖：—

---

## Wave 4 — 数据面 Tracing 大扫除（按目录分批 PR）

> 每个工单独立 PR，内部可并行。**#14 先完成再动这批**。
>
> **基建状态（2026-04-19）**：`PluginLog::push_err(kind: &'static str)` + `u_push_err` 已就绪（`src/core/gateway/observe/logs/buffer.rs` + `src/core/gateway/plugins/runtime/log.rs`），规范落 `skills/03-coding/observability/00-access-log.md` 「错误条目 API 约束」+ `skills/03-coding/01-log-safety.md`。**#10/#11/#12/#13 开工时直接用 `push_err("<static kind>")`**，不要自拼 `<plugin>:err:<kind>` 前缀，也不要 `push(&format!("err: {}", e))`。
>
> **legacy 存量并入 Wave 4 统一迁移**：`region_route/plugin.rs`（2 处 `err:failoverTo invalid; `）+ `rate_limit/plugin.rs`（1 处 `rl:err:cfg; `）+ `rate_limit_redis/plugin.rs`（3 处 `rl:err:*; `）—— 共 6 处旧无空格 `err:` 写法，在 #10~#13 对应插件被触达的那一把顺手改成 `push_err`。不单独开工单。

- [x] **#03** — Gateway HTTP proxy 回调 tracing（`pg_*.rs` 3 文件）— **S**
- [x] **#04** — TLS 握手 tracing（`tls/**` 4 文件）— **M** — 按用户决定不加 `edgion_tls_mtls_verify_total` / `edgion_tls_backend_cert_parse_total` 两个新 metric，ssl.log 的 `err_log` 已覆盖失败路径；顺带把 `parse_timeout.rs` 登记为配置期豁免（见 `skills/03-coding/01-log-safety.md` 例外表）
- [x] **#06** — backends/mod.rs tracing + SSRF（单文件 P0+P1 一次做）— **M** — 删 7 处 tracing（L302/322/376/386/771/975/980）+ L335 SAFETY 注释；SSRF 防御扩展：`classify_backend_addr` 覆盖 loopback + link-local（含 metadata `169.254.169.254` + IPv6 `fe80::/10`），明确不 block RFC1918；新增 `EdgionStatus::BackendAddrLinkLocalNotAllowed = 503_13` + 5 个新单测。按用户立场不加 `edgion_backend_port_resolve_total` / `edgion_backend_addr_rejected_total` / `edgion_backend_unsupported_kind_total` 三个 counter。遗留：`L573/L599` 的 `#[cfg(not(feature = "boringssl"))]` mTLS warn 未动（rustls 构建才触发，非默认，范围外）
- [ ] **#05** — 路由匹配 tracing + Controller validate 协同 — **M**
  - 注意：涉及 Controller validate 改动，可以和 Controller 侧 owner 协同
- [x] **#10** — OIDC 插件 tracing（33 处）— **M**
- [x] **#11** — JWT/JWE/Basic/HeaderCert/ForwardAuth tracing（10 处）— **M** — 删 10 处 tracing；JWT 按 `jsonwebtoken::errors::ErrorKind` 分 `jwt:expired / invalid-sig / malformed / bad-alg / ...` 等短标签（`verify_token` 接 `&mut PluginLog`），hide_credentials 限制改 `jwt:hide-skip-q / -c`；BasicAuth 统一 `basic:auth-fail;` 防 log-grep 枚举（timing 侧由 `auth_failure_delay_ms` 部署层缓解）；HeaderCert `hca:fail:<detail>` 带 static detail；ForwardAuth 新增 `EdgionStatus::ForwardAuthServiceUnreachable = 503_14` + PluginSession 窄接口 `add_error(EdgionStatus)` 双写 access log errors + `fa:unreachable(-degraded);` 标签。按用户立场不加 `edgion_auth_plugin_errors_total` counter。顺带：basic_auth 配置期 warn/info、header_cert_auth CA 加载 warn 登记到 `skills/03-coding/01-log-safety.md` 例外表。
- [x] **#12** — LDAP 命中率反模式 + tracing — **S**
- [ ] **#13** — 其他插件 tracing 杂项（17 处）— **M**
  - ⚠️ 同文件协同：**`ctx_set/plugin.rs:230` 若 #16 还未做就一起改**；**`rate_limit_redis/plugin.rs` 若 #17 还未做就一起改**
  - 依赖：**#14 先完成**

---

## Wave 5 — Metrics 整改

- [x] **#21** — BACKEND_REQUESTS_TOTAL label 可配置 + 默认极简 — **S**
  - 决策：引入 `MetricsConfig::backend_requests.labels` 开关（`gateway` / `route` / `backend` / `protocol` / `status` 五档，`*_namespace` + `*_name` 一对共用一个开关），默认仅 `gateway + protocol + status`（≈15 时序），`route` / `backend` 默认关，运维显式开启 drill-down。test_key / test_data 保留条件 push（非空才 push），并把 `init_integration_testing_mode` 的 `info!` 升级为 `warn!` 带生产 cardinality 警告。不走原 ticket 的 Cargo feature gate（逆项目运行时 flag 架构）
  - 变更：`gateway/cli/config.rs` 新增 `BackendRequestsConfig` + `BackendRequestsLabels` + `init_backend_requests_config` / `get_backend_requests_config`；`gateway/observe/metrics/registry.rs::record_backend_request` 改条件 push（签名加 `cfg: &BackendRequestsLabels`）；`routes/http/proxy_http/pg_logging.rs:153` 调用点同步；`gateway/cli/mod.rs:266` 启动初始化；`common/config/mod.rs::init_integration_testing_mode` 改 warn!
  - 文档：`skills/02-features/04-observability/01-metrics.md` 新增 "backend_requests label 配置" 小节 + cardinality 放大系数参考；`skills/03-coding/observability/01-metrics.md` 新增"带多维资源 label 的 counter：用配置项 + 条件 push"通用规则
- [x] **#22** — MIRROR_DURATION histogram 删除（以删代修）— **XS**
  - 决策：原 ticket 三个子问题（单位 ms vs 秒 bucket / enabled 复用 / 缺 set_buckets）都只影响 `MIRROR_DURATION_MS` 这一条 histogram，而该 histogram 自仓库诞生起单位就错、样本永远落 `+Inf`，生产上从未提供有效数据。mirror 是 fire-and-forget，tail latency 不影响用户流量；失败信号由 `MIRROR_REQUESTS_TOTAL{status="channel_full"|"timeout"}` counter 已经覆盖，per-request 时延 mirror access log 的 `et` 字段也有。直接删除比修复更正确
  - 变更：`registry.rs` 删 `MIRROR_DURATION_MS` const + `record_mirror_metric` 去掉 `duration_ms` 参数 + 删 histogram! 行；`plugins/http/request_mirror/plugin.rs:294` 调用点同步；`skills/03-coding/observability/01-metrics.md` 明文化"无 Histogram"规则的唯一例外（`edgion_gateway_request_duration_ms`）+ 补"细粒度观测走 access log 聚合"原则；`skills/02-features/04-observability/01-metrics.md` 指标表加 mirror counter 条目 + 观测分工速查表
  - 同文件协同：#21 仍在 `gateway/observe/metrics/registry.rs` 未做

---

## 同文件协同速查表

| 文件 | 涉及工单 | 建议 |
|------|---------|------|
| `gateway/plugins/http/ctx_set/plugin.rs:230` | #13 + #16 | #16 先做（含删 L230），#13 跳过该行 |
| `gateway/plugins/http/rate_limit_redis/plugin.rs` | ~~#17~~ + #13 | #17 已完成；#13 剩余 tracing 行 L193/L495 删除，与 #17 改过的 L491-571 PluginLog 无重叠 |
| `gateway/plugins/runtime/plugin_runtime.rs` | #13 + #14 | **#14 必须先** |
| `center/fed_sync/server/mod.rs` | #18 + #19 + #20 | 顺序 #18 → #19 → #20 |
| `center/fed_sync/registry/mod.rs` | #19 + #20 | 顺序 #19 → #20 |
| `gateway/observe/metrics/registry.rs` | ~~#21~~ + ~~#22~~ + ~~#23~~ | 全部完成（#21 改条件 push + 5 档 label 开关，#22 以删代修，#23 新增 truncated counter） |
| `center/api/mod.rs` | #01 + #20 | 不同函数，并行安全 |

---

## 进度汇总（可选：每次勾选后手动更新）

- Wave 1：10 / 10 ✅
- Wave 2：4 / 4 ✅（#24 拆为 #24a + #24b，两个子工单皆已落盘）
- Wave 3：4 / 5
- Wave 4：6 / 8
- Wave 5：2 / 2 ✅（#21 改条件 push，#22 以删代修）
- **总计：26 / 29**

---

## 遇到问题怎么办

- **工单步骤不清楚** → 打开对应 `NN-*.md` 看详细步骤
- **想查证据 / 触发条件** → 工单里"溯源" 列的 ID 可查对应报告（`01-controller-core.md` 等），但这些报告可能已在本地被删，需从 git history 恢复：
  ```bash
  git log --all --oneline -- tasks/reviews/2026-04-17/01-controller-core.md
  git show <commit>:tasks/reviews/2026-04-17/01-controller-core.md
  ```
- **发现新问题 / 工单过时** → 直接改对应 fix 文件，不要开新 issue（保持单一索引）
- **工单描述错误** → 标记为 `- [~]`（skip）并注明原因，不要直接删 checkbox
