# TODO: SSL-08 剩余低优先级问题

> 来源: `tasks/review/ssl/08-silent-failures-and-inefficiencies.md`

## 已完成

- **Cipher 静默失败** — `configure_ciphers` 两处 `Ok(())` 改为返回 `Err`，错误写入 ssl.log
- **tls_id 碰撞** — `u32` → `u64`，时间戳不再截断，格式改为 `{:016x}-{:016x}`

## 搁置（不紧急）

### 问题 2：GatewayBackendTlsStore TOCTOU

`set`/`remove` 用 `load/clone/store` 而非 `rcu()`。
**实际无风险**：写操作只在 Gateway reconcile 单任务上下文中顺序调用，不存在并发写。
修复方式：将 `set`/`remove` 改为 `self.store.rcu(|cur| { ... })`，改动约 10 行。

### 问题 3：证书双重解析

`validate_cert` 中 X.509 被解析两次（一次取有效期，一次取 SAN）。
**实际无影响**：仅在配置变更时调用，非握手热路径。
修复方式：合并为单次解析，同时提取有效期和 SAN，需调整函数签名。
