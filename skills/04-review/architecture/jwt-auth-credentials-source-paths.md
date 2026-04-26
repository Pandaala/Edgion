---
name: jwt-auth-credentials-source-paths
description: 判定 jwt_auth `load_credentials_from_secret` "4 路重复 / 应抽 helper" 类 finding。已审计的设计取舍，不修。
---

# jwt_auth credentials.rs 4 路凭证加载 — 已审计的设计取舍

**审计日期**：2026-04-26
**关联 issue**：P2-12（已关闭，未修复）
**结论**：**`src/core/gateway/plugins/http/jwt_auth/credentials.rs::JwtAuth::load_credentials_from_secret` 的 4 个分支保持现状，不接受"抽 helper / 消除重复 / 缩短主函数"类修复建议。**

## 涉及代码

- `src/core/gateway/plugins/http/jwt_auth/credentials.rs:66-186`
  - 分支 1：`config.resolved_credential`（controller 预解析的单凭证）
  - 分支 2：`config.resolved_credentials`（controller 预解析的多凭证 map）
  - 分支 3：`config.secret_ref`（fallback：从 K8s Secret 拿单凭证）
  - 分支 4：`config.secret_refs`（fallback：从 K8s Secret 拿多凭证）

## 看上去像问题的点（不修）

4 条路径都执行 symmetric/asymmetric 二分 + base64 解码 + UTF-8/PEM 提取 + 错误信息构造，函数 ~120 行。表面上是教科书式的"应抽 helper"。

## 为什么不改

1. **"重复"是结构性而非真重复**：4 条路径数据源类型不同
   - resolved 路径：`String`（已 base64 编码，asymmetric 分支只 `pem.clone()`）
   - secret 路径：`Vec<u8>`（K8s Secret 原始字节，asymmetric 分支要走 UTF-8 转换）
   - 错误信息标签格式各异（"Resolved credential" vs "Secret {ns}/{name}" vs "Resolved credential '{key}'"）
   - 抽 helper 必然要参数化"如何取数据 + 错误标签格式"，helper 本身复杂，节省的代码 = 几十行，引入的间接 = 一层

2. **不在热路径**：`load_credentials_from_secret` 被 `load_credentials` 缓存（`self.credentials: RwLock<Option<Credentials>>`），整个 JwtAuth 实例只调一次。性能、可读性诉求都很弱。

3. **回归风险实在**：错误信息文案被 25 个集成测试逐字符校验（`tests/secret_decoding.rs`、`tests/algorithm.rs` 等）。重构收益是几十行代码，代价是认真对齐错误文案 + 跑全套测试。

4. **结构稳定**：4 路由 schema 决定（resolved 单/多、fallback 单/多），不会再加第 5 路。"未来扩展" 动机不成立。

## 适用判定规则

未来 review 看到以下 finding，直接关闭、不重新评估：

- "`load_credentials_from_secret` 4 路重复，应抽 `build_credential_from_*` helper"
- "120 行函数太长，应拆分"
- "symmetric/asymmetric 二分写了 4 次，违反 DRY"

如果发现的是**新维度**（例如：4 路在某种边界条件下行为不一致、controller-resolved 与 fallback get_secret 路径有未文档化的语义差异），那是另一个 finding，按问题本身评估。
