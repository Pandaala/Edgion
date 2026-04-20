# 16 — ctx_map 敏感数据横向泄漏 + denylist append 语义

**等级：** P1
**修复类型：** 设计改动（私有 ctx 命名空间 + denylist append）
**工作量：** L（短期 S）
**溯源：** P1-Plugin-02 + P2-Plugin-11 + D-N1 + D-N2 + D-N3

## 位置

**存敏感数据的插件：**
- `src/core/gateway/plugins/http/jwt_auth/plugin.rs:542` — `set_ctx_var("jwt_claims", claims_json)`
- `src/core/gateway/plugins/http/openid_connect/openid_impl.rs:721` — `set_ctx_var("oidc_claims", &claims_json)`
- `src/core/gateway/plugins/http/jwe_decrypt/plugin.rs:379` — `set_ctx_var("jwe_payload", &plaintext)`

**访问/过滤层：**
- `src/core/gateway/plugins/runtime/session_adapter.rs:294-296`（跨插件 `get_ctx_var` 无约束）
- `src/types/ctx.rs:699-701`（`_` 前缀仅影响 access log）
- `src/core/gateway/cli/config.rs:582,597-607`（`ctx_log_denylist` replace 语义）
- `src/core/gateway/observe/access_log/entry.rs:51,79`（`stage_logs` 序列化无过滤）

**CtxSet debug 泄漏：**
- `src/core/gateway/plugins/http/ctx_set/plugin.rs:230` — `debug!("... = '{}'", v)` 把 ctx 值拼进 log

## 问题

1. **ctx_map 存完整敏感 JSON**：JWT/OIDC Claims / JWE 明文（含 PII、PAN）
2. **跨插件横向泄漏**：其他插件 `${ctx:jwt_claims}` 可读，恶意 CtxSet 或 RequestHeaderModifier 配置可把 claims 拷到上游 header
3. **`_` 私有约定只在 access log 末端生效**：跨插件 `get_ctx_var("_foo")` 完全畅通
4. **`ctx_log_denylist` replace 语义**：用户 override = 清空默认（含 claim/payload/token），一次配错就吐 JWT
5. **`stage_logs` 零过滤**：PluginLog 推进去的任何内容直接序列化到 access log
6. **CtxSet debug 泄漏**：ctx 值拼进 tracing log

## 修复步骤

**短期（S）：**

1. **删除 `ctx_set/plugin.rs:230`**（工单 #13 已列）
2. **`ctx_log_denylist` 改 append 语义**：
   - 新增 `ctx_log_denylist_extra: Vec<String>`（append 到默认）
   - 老字段 `ctx_log_denylist` 标 deprecated（仍保持 replace，发 warn）
   - 默认 denylist 继续包含 `claim` / `payload` / `token` 等
3. **`stage_logs` 加过滤**：序列化前扫 PluginLog 内容中的敏感子串（token/password/secret/claim），命中则替换为 `<redacted>`
4. **文档警示**：`store_claims_in_ctx` 配置 description 明确"该 key 会被同请求其他插件读取"

**长期（L）：**

5. **私有 ctx 命名空间**：
   - 方案 A：`set_ctx_var_with(key, val, CtxVisibility::Private)` → `get_ctx_var` 跨插件读 private key 返回 None
   - 方案 B：约定 `_` 前缀 = 私有 + CtxSet 配置层拒绝引用 `_` 前缀
6. **弃用 `store_claims_in_ctx`**：强制走 `claims_to_headers` 映射（已存在的设计意图）
7. **JWE `store_payload_in_ctx`**：改为只存 SHA-256 fingerprint 或 claim-level 解构

## 验收

- [ ] `cargo check --all-targets` 通过
- [ ] 单测：`ctx_log_denylist_extra = ["custom"]` + 不设 `ctx_log_denylist` → 默认 denylist 生效 + custom 追加
- [ ] 单测：stage_logs 包含 `token=xxx` 片段 → access log 输出 `<redacted>`
- [ ] 单测：私有 ctx（短期用 `_` 前缀）跨插件读 → 取不到
- [ ] CtxSet 测试不再在 log 中看到 ctx 值
- [ ] 集成测试：JWT claims 不经 access log / 上游 header 泄漏
