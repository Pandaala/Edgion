# TODO: redis.* builtin 错误 / 连接失败路径覆盖（需先回答可行性问题）

> 状态：**待 brainstorm**。原 `tasks/todo/dsl-vm-rhai-research/issues/issue-18-redis-builtin-error-coverage.md` 因影响面过大（跨模块、新增公共 trait、与生产设计不一致的验收标准）从 `/fix-issue` 升级为独立 todo；启动前必须先回答下面 §「待回答的根本性问题」三条，否则不开工。

---

## 来源

- 原 issue（已删除）: `tasks/todo/dsl-vm-rhai-research/issues/issue-18-redis-builtin-error-coverage.md`
- 父调研报告: [`dsl-vm-rhai-research/05-test-coverage.md`](dsl-vm-rhai-research/05-test-coverage.md) §1.2 covers matrix（line 154-167），§2 汇总：71 个 BuiltinId 中 13 个 redis.* 完全无单测，占总缺口 81%
- 原 issue 优先级 P2，预估 1.5–2 d（含 mock 基础设施）

## 原 issue 要点摘要（保留以免重复挖掘）

| 项 | 内容 |
|---|---|
| 缺口 | 13 个 `redis.*` builtin（`RedisGet/Set/Del/Exists/Ttl/Expire/Incr/IncrBy/HGet/HSet/HDel/HGetAll/HExists`）在单测层零覆盖 |
| 障碍 | `RedisLinkClient`（`link_sys/providers/redis/client.rs`）直接持有 `fred::clients::Pool`，无可注入接口 |
| 原推荐方案 | trait inject + mockall：在 `link_sys/providers/redis/` 引入 `pub trait RedisExec`；`RedisLinkClient` 实现该 trait；`#[cfg(test)] MockRedisExec`（automock）；`tests/mod.rs` 加 `run_with_redis_mock(...)` helper |
| 备选 | (2) `redis-protocol` in-memory server；(3) `mini-redis` dev-dep |
| 原验收 | ≥26 test fn（13 × happy + error），+ 1 个连接失败路径 |
| 前置依赖 | issue-15（`run_expect_runtime_err` helper）—— **已落地**，无外部 blocker |

## 待回答的根本性问题（开工前必须有结论）

### Q1. 这件事是否真的有必要？

正方：
- 单测层 0 覆盖，未来回归（如新加的 `RedisHGetAll` `max_map_size` 上限）只能靠集成测试发现，反馈慢
- 错误映射 (`anyhow::anyhow!("Redis GET {}: {:?}", key, e)` → `redis_err(e.to_string())`) 是字符串拼接，没有静态保证，不测就漂移
- 71 个 builtin 缺口里 redis 占 81%，覆盖率指标层面也突兀

反方（必须认真评估）：
- 实际错误路径在 `examples/test/scripts/integration/` 里跑真 Redis 时就已经被执行；线上回归不会因为缺单测而漏
- `call_builtin_redis` 13 条分支结构高度同质（`pop → resolve → call → map_result`），单测只验证「映射没写错」，回报有限
- 加 trait 抽象本身就是新的复杂度来源（参见 Q2）；可能用更复杂的代码换来不重要的测试
- **替代方案**：是否只需在 `tests/redis_builtins.rs` 加 2 个 mock-free 用例覆盖 `resolve_redis_client` 的两条 `Err(redis_err(...))` 分支（"link not found" / "link not ready"），其余依赖集成测试？这能用 1–2 h 拿到「连接失败」覆盖，剩余成本归零

**决策需要**：拿到「跳过 trait 重构 + 只测 resolve 失败两路」「全量 mock + 26 用例」「彻底不做」三档的实际收益对比，再决定。

### Q2. Redis 的设计是否过于复杂？

观察：
- `link_sys/providers/redis/`：client.rs 165 行 + ops.rs 437 行（24 个 pub 方法）+ config_mapping.rs 477 行
- DSL 这一侧：`call_builtin_redis` 又把 ops 里的 13 个方法重新包了一层（`builtins.rs:995-1232`，每条分支结构几乎一样）
- 错误模型分两段：业务层用 `anyhow::Result` + `anyhow!("Redis XXX {}: {:?}", ...)`，DSL 层 `e.to_string()` 后塞进 `redis_err(...)` Map —— 中间层信息（原 fred error 类型）已经丢失
- `RedisLinkClient` 的健康标志是 `Arc<AtomicBool>` + on_reconnect / on_error 回调，但 13 个 ops 方法没人查 `healthy()`；DSL 层也没查 —— 健康标志的意义未被消费

**问题**：
- ops.rs 把 fred 高层接口（已经是 typed）又重包一遍主要换什么？除了把 `RedisError` 转 `anyhow::Error` 之外，有无别的价值？
- 13 个 builtin 各自写一条 case arm，相对于「一个通用的 `redis.cmd(link, "GET", key)`」+ 解析返回，复杂度是否值？（见 Q3）
- 引入新的 `RedisExec` trait 是否在「已经过厚」的栈上再加一层？

**决策需要**：评估「ops.rs 这一层是否值得保留」，再决定 mock 该插在哪一层（fred 层 / ops 层 / 新 trait 层）。

### Q3. 用 redis 命令（通用入口）会不会更简单？

可能的「更简单」形态有两种，得分清：

**(a) DSL 表面统一为 `redis.cmd(link, "GET", key, ...)`（单一通用 builtin）**
- 收益：13 个 `BuiltinId::Redis*` → 1 个 `BuiltinId::RedisCmd`；`call_builtin_redis` 13 条分支 → 1 条；bytecode.rs:113-241 大幅缩减；新 redis 命令零代码增改
- 代价：失去编译期参数校验（参数个数/类型，现在 emit 阶段就能挡）；脚本作者错拼命令名得运行时才报；类型转换（HGetAll 的 Map 结果）要在运行时按命令名分派 —— 复杂度从 builtins.rs 转到一个新的「命令注册表」
- 安全模型变化：现在的 13 个 builtin 是 allowlist，`redis.cmd(...)` 默认就是 allowlist=*；要恢复同等粒度得在 validator 里做命令白名单
- 与现有 DSL builtin 风格（每个动作一个具名 builtin）冲突，需评估一致性损失

**(b) 业务层直接用 `client.pool().cmd::<...>(...)`，绕过 ops.rs 的 24 个包装**
- 收益：删 ops.rs ~437 行，DSL 这一侧直接面向 fred；mock 时直接 mock fred 接口或者用 `redis-protocol` 假服务器
- 代价：fred error 类型直接渗透到 DSL builtins.rs；anyhow 转换的「Redis GET {key}: ...」上下文得在调用点重做；DSL 之外的 ops 调用方（如果有）也得改

**问题**：(a) 和 (b) 不互斥。要先想清楚：现在 ops.rs 这层到底服务谁、是否能撤掉，再判定 DSL 这层是否要做 (a)。

**决策需要**：先回答 Q2，再决定是否走 (a) / (b) / (a)+(b) / 都不动。

## 验收标准的已知修正

原 issue 的验收标准有一条与生产设计矛盾，必须重写：

> ❌ 原文：所有错误断言使用 `matches!(err, RuntimeError::ApiError { function, .. } if function == "redis.get()")`

事实：`call_builtin_redis`（`builtins.rs:986-994` 注释 + 全部 13 条分支）**故意永远不**抛出 `RuntimeError`。所有失败路径（包括 `resolve_redis_client` 失败、fred 报错、TTL 类型错误、参数个数错）都返回 `Ok(redis_err(msg))` —— 即 `Map { ok: false, value: Nil, error: Str(msg) }`，由脚本作者决定 fail-open / fail-close / 优雅降级。

正确断言应为：

```rust
// run 出来的 Value 是 Map，从 "ok" / "error" 字段断言
match result {
    Value::Map(m) => {
        assert_eq!(m.get("ok"), Some(&Value::Bool(false)));
        assert!(matches!(m.get("error"), Some(Value::Str(s)) if s.contains("...")));
    }
    other => panic!("expected Map, got {:?}", other),
}
```

如果决定走「全量 mock + 26 用例」方案，先在测试 helper 里封装 `assert_redis_err(result, expected_substring)` 之类的工具，避免 26 个 case 各写一遍。

## 候选实施路径（brainstorm 时取舍）

| 路径 | 工作量 | 覆盖收益 | 引入复杂度 | 适用前提 |
|---|---|---|---|---|
| **P0**：只补 2 个 `resolve_redis_client` 失败用例（mock-free） | 1–2 h | 仅「link not found / not ready」两条路径 | 无 | Q1 答「单测有用但不必全量」 |
| **P1**：trait inject (`RedisExec` + mockall)，写满 26 用例 | 1.5–2 d | 全 13 builtin happy + error | 新公共 trait + 新 dev-dep（mockall 已在依赖里则 0） | Q2 答「ops.rs 该层保留 + Q3 走 (b)」 |
| **P2**：撤掉 ops.rs，DSL 直接对 fred；mock 用 `redis-protocol` 假服务器 | 3–5 d | 同 P1 但路径更短 | 删 437 行 + 新 mock 基础设施 | Q2 答「ops.rs 该撤」 |
| **P3**：DSL 表面统一为 `redis.cmd(link, cmd, args...)` + validator 白名单 | 3–5 d | 13 用例 → 少量但更通用 | 失去编译期参数校验，与现有 builtin 风格冲突 | Q3 答「值得换通用入口」 |
| **P4**：什么都不做，记录在 04-review 作为「accepted gap」 | 5 min | 0 | 0 | Q1 答「集成测试已覆盖，单测层 0 可接受」 |

P0 与 P1/P2/P3 互不冲突，可作为 brainstorm 后的「至少要做到这一步」兜底。

## 不在本任务范围

- 本 task 只覆盖错误路径 / 连接失败 / 行为契约的**单测**层；happy path 的「读写一致性、TTL 真实计时」属于集成测试，归 `examples/test/scripts/integration/`
- 与 fred 升级、Redis Cluster Sentinel 切换语义、TLS 后端测试无关
- 不涉及 `link_sys/providers/redis/config_mapping.rs`（CRD → fred Config 映射）的测试

## 下一步

1. brainstorm 回答 Q1 / Q2 / Q3，选定 P0–P4 中一档（或组合）
2. 选定后再决定是否写 spec、是否单独开「ops.rs 该层去留」的 ADR
3. 任何时候若决定「不做」，把结论写到 `skills/04-review/observability/` 或 `skills/04-review/architecture/` 下，避免未来 review 反复报告同一缺口
