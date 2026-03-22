# 代码质量与性能优化

优先级: P2

## 1. GRPCRoute match engine 避免不必要的 clone

### 位置

`src/core/gateway/routes/grpc/match_engine.rs:94`

### 问题

每次请求都执行 `service.clone()` + `method.clone()` 用于 HashMap 查找。

### 建议

使用 borrowed key 查找，避免每次请求分配字符串：
- 为 HashMap key 实现自定义 `Borrow` trait
- 或使用 `HashMap::get` 接受 `&str` 元组的方式

---

## 2. TLS proxy 双向拷贝简化

### 位置

`src/core/gateway/routes/tls/proxy.rs:433-491`

### 问题

手动实现 `select!` + 8KB buffer 的双向字节拷贝，代码复杂且性能不如标准库。

### 建议

替换为 `tokio::io::copy_bidirectional`，更简洁、性能更好（内部使用更大的缓冲区和零拷贝优化）。

---

## 3. EdgionTls 未实现的扩展字段

### 位置

`src/types/resources/edgion_tls.rs`

### 问题

以下字段有数据模型定义但无运行时接入：
- OCSP stapling
- Session tickets
- Session cache
- Early data (0-RTT)
- Revocation checks

用户可以配置这些字段但不会生效，容易造成困惑。

### 建议

- 短期：在文档中标注为 "reserved / 未实现"，配置验证时发出 warning
- 长期：逐步实现或移除

---

## 4. Weighted selector 全零 weight panic

### 位置

`src/core/gateway/lb/weighted_selector.rs`

### 问题

所有 backend weight = 0 时，`assert!(total_weight > 0)` 会 panic。

### 建议

- 优雅处理：若所有 weight 为 0，返回 503（无可用后端）
- Gateway API 规范说 weight 0 = 不发送流量，全零意味着无可用目标

---

## 5. BackendTLSPolicyStore target key 精度不足

### 位置

`src/core/gateway/backend/backend_tls_policy_store.rs`

### 问题

`build_target_key` 只使用 `namespace/name`，忽略了 `group` 和 `kind`。若同一 namespace 中不同类型的资源同名，可能匹配到错误的 policy。

### 建议

target key 格式改为 `namespace/group/kind/name`，与代码注释保持一致。
