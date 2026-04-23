# `EdgionTls.min_tls_version` 允许 `TLS1_0` / `TLS1_1`：刻意的 opt-in，不拒

## 结论

`EdgionTls` CRD 的 `min_tls_version` 字段接受 `TLS1_0` / `TLS1_1` 是**刻意的 opt-in 设计**，为遗留客户端场景（老 IoT、支付终端、离线 legacy 系统）保留的通道。**不应**要求在 Controller 验证器中返回 validation error、也不应通过 `#[cfg]` 在 release 构建中拒绝。operator 自行承担选择不安全版本的责任。

## 原因

1. **默认路径已安全**：不设置 `min_tls_version` 时 BoringSSL 默认 TLS 1.2+。只有 operator **显式**选择 `TLS1_0` / `TLS1_1` 才暴露攻击面——这是知情选择，不是默认漂移
2. **用户文档已明确警告**（`docs/{zh-CN,en}/ops-guide/gateway/tls/edgion-tls.md` 的"支持的版本"表）：
   - `TLS1_0` / `TLS1_1` → "⚠️ 不推荐（已废弃）"
   - `TLS1_2` → "✅ 推荐"
   - `TLS1_3` → "✅ 最安全"
3. **Rust 端 doc comment 同步警告**：`TlsVersion` 枚举和 `min_tls_version` 字段 doc 明确引用 RFC 8996，建议 `TLS1_2` 作为 baseline；通过 JSON schema 暴露给 `kubectl explain` / schema viewer
4. **行业对照一致**：Nginx `ssl_protocols` 不限制；Envoy `tls_params.tls_minimum_protocol_version` 不限制；拒绝发生在**控制面 / 产品层**（Istio、云 LB）而非代理引擎本身。Edgion 作为代理引擎保持一致姿态
5. **硬拒的代价 > 防御收益**：
   - 切断所有合法遗留部署（无逃生门）
   - operator 错选代价落在自己的客户端连接上（可见、可审计），不会静默蔓延到其他组件
   - 强制移除等同于 Edgion 单方面声明"不支持"某个合法业务场景

## 判定规则

当 review / 审计报告出现以下模式时，按"operator 知情选择"关闭：

1. "TLS 1.0 / 1.1 可被 operator 配置，应在 validator 中拒绝"
2. "应通过 `#[cfg(not(debug_assertions))]` 在 release 构建拒绝 TLS 1.0/1.1"
3. "RFC 8996 已 deprecate，应强制移除 TLS 1.0/1.1 枚举变体"
4. "应在 annotation 级别 opt-in 启用遗留 TLS（当前太宽松）"

验证三点再关闭：

- `TlsVersion` 枚举仍含 `Tls10` / `Tls11` 变体，未被 `#[cfg]` 排除
- `EdgionTlsHandler::validate()` 未对 `min_tls_version = Tls10 | Tls11` 返回 error
- `docs/{zh-CN,en}/ops-guide/gateway/tls/edgion-tls.md` "支持的版本"表仍含明确的 ⚠️ 标注；`edgion_tls.rs` 的 `TlsVersion` / `min_tls_version` doc comment 仍引用 RFC 8996 deprecation

三点满足 → 刻意设计，指向 `tasks/security-audit/05-tls-cert-acme.md` M19 NOT-AN-ISSUE 决策关闭。

## 例外场景（需重评）

- 行业最佳实践迁移到"代理引擎层默认拒绝 TLS 1.0/1.1"（如 Envoy 未来某版本硬拒）→ 重评是否跟随
- 出现多起"operator 无意中配 TLS1_0 导致合规事件"反馈 → 考虑新增 opt-in annotation 机制（`edgion.io/allow-insecure-tls-version: "true"`），把默认改成拒绝
- 加入合规认证（PCI-DSS 4.0 / FIPS 140-3）要求代理层强制拒绝 → 按认证要求硬拒

## 与同类 opt-in review 判定的交叉引用

- `observability/system-error-state-metric.md`（M2 `skip_tls`）——保留逃生门 + 可观测兜底
- `real-ip/max-trusted-hops-opt-in.md`（M12）——opt-in 增强 + 主防线在配置而非代码
- 本文件（M19）——opt-in 降级 + 代码默认安全 + 文档多层警告

三者共同模式：**默认安全 + 保留 opt-in 通道 + 多层显式警告 + operator 自担责**，用于"既不能一刀切拒绝、也不能放任默认漂移"的场景。

## 背景记录

- 决策日期：2026-04-22
- 决策来源：`tasks/security-audit/05-tls-cert-acme.md` M19
- 本次改动：仅 Rust 端 doc comment 增强，零代码行为变更
- 参考：RFC 8996（2021-03 deprecate TLS 1.0/1.1）、Chrome 84 / Firefox 78 / Safari 14 / Edge 84 客户端移除
