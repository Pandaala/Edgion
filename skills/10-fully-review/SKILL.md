---
name: fully-review
description: Edgion 全局代码 Review 指南。包含 AI 执行全量 review 的结构化流程、各维度检查清单、已确认的误报和关闭条目（下次 review 直接跳过）。
---

# Edgion 全局 Review 指南

> **使用场景**：对 Edgion 代码库进行全局安全/质量 review，或评审新提交的 review 报告。
> **执行顺序**：先读本文件 → 按需加载各维度 → 对照关闭条目跳过已知误报。

---

## 执行流程

```
1. 加载已有 review 知识（04-review/）
2. 对照「已关闭条目」跳过已知误报
3. 按以下维度逐项检查
4. 每个发现：先核实可利用路径，再定级
```

---

## 维度一：安全

### 通用原则

在标记任何安全问题前，必须回答：

1. **攻击路径是否可达**？攻击者需要什么前提条件？
2. **是否有独立熵阻断攻击**？（见下方 Timing Attack 判定规则）
3. **修复是否治根**？弱密码格式的根本问题是算法弱，不是比较方式。

### Timing Attack 判定规则

不能仅凭"使用了非恒定时间比较"直接定为高危，必须验证：

| 检查项 | 说明 |
|--------|------|
| 被比较的值是否含独立随机熵 | 若有 CSPRNG nonce 且验证时需全部字段匹配，计时 oracle 无法利用 |
| 网络抖动是否淹没时间差 | 网关场景下毫秒级抖动远大于纳秒级短路差，实测信噪比极低 |
| 攻击者是否能控制输入并大量重试 | 有速率限制或 token 一次性消耗时不可利用 |

**典型误判**：CSRF token 的签名比较。token 包含 CSPRNG random（256-bit），即使能猜出 sign，也无法与未知 random 配对，计时攻击无法利用。

### 弱密码格式

弱哈希（SHA-1、MD5）的根本问题是**无 key-stretching**，加常时间比较是治标。正确处置：

- 弱格式的实际 threat 是离线暴力破解，不是计时侧信道
- 常时间修复对已经弱的哈希收益极低，优先级应降低
- 正确行动：文档明确推荐 bcrypt，运维层面迁移；弱格式警告放在控制面（Controller 加载配置时），不在数据面热路径

### 密码/密钥缓存键

将明文或可逆编码（base64）存为 LRU cache key 的内存泄露风险：

- 需结合 **cache value** 一起分析：若 value 是 username，cache key 碰撞会导致认证错人，比内存泄露更严重
- 哈希 key 方案（SHA-256/ahash）需能保证无碰撞，否则引入认证绕过
- CRC32 碰撞概率在千用户规模不可接受，不适合用作认证 cache key
- 若内存安全不是合规强制要求，该问题可降为观察项

---

## 维度二：正确性

- Controller 资源处理 requeue 规则 → [04-review/handler-requeue/](../04-review/handler-requeue/handler-requeue-rule.md)
- 内存"泄漏"判定 → [04-review/memory-leak/](../04-review/memory-leak/not-a-memory-leak.md)
- 路由架构一致性 → [04-review/01-route-architecture-consistency.md](../04-review/01-route-architecture-consistency.md)
- LB 计数器：跨 retry 的 LeastConn 计数递增/递减对称性

---

## 维度三：可观测性

完整规则 → [04-review/observability/](../04-review/observability/observability-review-rule.md)

快速检查：

- [ ] 数据面热路径无 `tracing::*`
- [ ] PluginLog ≤ 40 字节，只记最终决策，不含敏感值
- [ ] Metrics 无 Histogram，Label cardinality 有界
- [ ] `ctx_map` 无原始 JWT claims / 密码

---

## 维度四：架构分层

- 弱密码检测、格式警告 → **Controller 层**（配置加载时），不在数据面
- 配置校验、Schema 合法性 → Controller
- 请求路由、插件执行、认证 → Gateway 数据面
- 数据面若需运营警告，通过 PluginLog → access log 表达，不用 `tracing::warn!`

---

## 已关闭条目（Review 时直接跳过）

下表记录经过深度分析后确认不需要修复的条目。再次 review 时如遇到相同问题，参考此处结论直接关闭。

| ID | 模块 | 问题描述 | 结论 | 关闭原因 |
|----|------|---------|------|---------|
| P0-04 | `csrf/token.rs` | `self.sign != expected_sign` 非恒定时间比较；`SHA-256(expires+random+key)` 非 HMAC | **关闭** | token 含 256-bit CSPRNG random，计时 oracle 无法与 random 配对利用；Length extension 因 exact-match 验证无可利用路径 |
| P0-04b | `csrf/token.rs` | key mixing 使用 SHA-256 而非 HMAC | **关闭** | 同上，验证逻辑为 exact-match，Length extension 构造的变体无法通过 `gen_sign` 格式验证 |
| P0-05a | `basic_auth/htpasswd.rs` | SHA-1 / MD5-crypt 比较非恒定时间 | **关闭** | 根本问题是算法弱（无 key-stretching），常时间修复收益极低；网络抖动远大于比较时差；真正的行动是迁移到 bcrypt |
| P0-05b | `basic_auth/plugin.rs` | LRU cache key 存 `base64(user:pass)` | **关闭** | 哈希 key 方案引入碰撞→认证错人风险；内存安全非当前合规要求；若未来需支持，加可选 config 项，key hash 用 ahash（非 CRC32） |
| P0-09 | `csrf/token.rs` | `SystemTime::now().duration_since(UNIX_EPOCH).unwrap()` 在热路径（DoS 风险） | **关闭** | 触发条件是系统时钟早于 1970，实践中不可达（NTP 回拨/VM 漂移均不会回拨 56 年；`SystemTime` 返回 UTC，不受时区影响）。**建议的 `unwrap_or(Duration::from_secs(0))` 修复反而有安全副作用**：verify 时 `now=0`，`0 - expires`（正数）为负，永远 < max_expires，过期检查被静默绕过，CSRF 保护失效。panic 暴露问题，比静默安全降级更安全。 |
