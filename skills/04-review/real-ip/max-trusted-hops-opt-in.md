# RealIp `maxTrustedHops` opt-in：设计决策

## 结论

`RealIpConfig.max_trusted_hops` 是 **opt-in 可选字段**，默认 `None`（Nginx 兼容行为，无上限）。这是 2026-04-22 对 M12 的刻意取舍——提供 Envoy 风格的跳数上限作为辅助防御，但**不**强制启用，也**不**改默认为某个具体数字。

遇到 review 报"RealIp 应把 `maxTrustedHops` 改为必填 / 设定保守默认值 / 移除该字段"时，按下述规则判定。

## 背景威胁（M12）

`recursive=true` 模式下，右→左遍历 `X-Forwarded-For` 列表直到遇到首个非可信 IP。若 `trusted_ips` 配得过宽（如 `10.0.0.0/8`）且攻击者能从该范围内的 IP 发起请求，可以通过构造任意长度的"可信 IP 前缀"把恶意 IP 推至最左端，让 `client_ip` 被伪造。

`maxTrustedHops: Some(N)` 在递归遍历到第 N 个可信 hop 时停止，返回当前位置 IP——堵死"无限延长可信前缀"这条路。

## 设计约束

1. **默认 `None`（无上限）**：保持 Nginx 兼容，升级部署零破坏
2. **对齐 Envoy `xff_num_trusted_hops` 概念**：给 Envoy-familiar operator 熟悉的控制面
3. **仅对 `recursive=true` 有意义**：非递归模式下字段存在但无效
4. **主防御在 `trusted_ips`**：`maxTrustedHops` 是辅助防线，不是主防线；文档明确"双层防御"
5. **字段类型 `Option<u32>`**：`None` = 无上限；`Some(0)` 合法但语义等于"不跳过任何可信 hop"（实际相当于非递归语义）

## 判定规则

当 review 报告出现以下模式时，按"设计决策"关闭：

1. "`maxTrustedHops` 应改为必填，防止 operator 遗漏"
2. "应把默认值从 `None` 改为 `5`（或其他具体数字）作为保守默认"
3. "应删除 `maxTrustedHops`，由代码强制单 hop"
4. "非递归模式下 `maxTrustedHops` 应报错（当前是忽略）"

验证三点再关闭：

- `RealIpConfig.max_trusted_hops` 字段类型仍是 `Option<u32>`，默认 `None`
- `RealIpExtractor::resolve_from_header` recursive 分支仍用 counter 实现 cap（非递归分支不读此字段）
- 用户文档 `docs/{zh-CN,en}/user-guide/edgion-plugins/real-ip.md` "安全取舍" / "Security Trade-offs" 小节仍在，明确"第一道防线是精确 `trustedIps`、第二道是 `maxTrustedHops`"

三点满足 → 设计决策，指向 `tasks/security-audit/02-access-control.md` M12 FIXED 条目关闭。

## 例外场景（需重评）

- 行业最佳实践统一迁移到"强制 hop cap"（Envoy 已是 required）→ 评估是否把 Edgion 也改成必填
- 出现大量"operator 因没配 `maxTrustedHops` 被攻击"的安全事件反馈 → 考虑改保守默认（伴随 breaking change 文档）
- 加入 `recursive: false + maxTrustedHops` 组合的 validator error（当前是静默忽略）

## 与其他 opt-in 安全增强的同类判定

本决策和以下 review 规则同属"**Edgion 自创语法糖/可选项，保留默认行为 + opt-in 增强**"模式，判定方式一致：

- `cors/subdomain-wildcard-https-only.md`（M13）——不同之处：M13 是 breaking 修复（改了简写语义），M12 是 non-breaking opt-in
- `oidc/nonce-pkce-defaults.md`（M8）——M8 doc-only，M12 代码+文档

## 背景记录

- 决策日期：2026-04-22
- 决策来源：`tasks/security-audit/02-access-control.md` M12（Option B — code + 文档）
- 实现文件：
  - `src/types/resources/edgion_plugins/plugin_configs/real_ip.rs`
  - `src/core/common/utils/real_ip_extractor.rs`
- 新增测试：`test_max_trusted_hops_caps_recursive_traversal`、`test_max_trusted_hops_does_not_block_real_client_within_window`、`test_max_trusted_hops_ignored_in_non_recursive_mode`
