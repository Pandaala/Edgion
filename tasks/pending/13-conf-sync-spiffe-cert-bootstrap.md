# 13 - conf_sync mTLS 证书自动签发 / SPIFFE 身份引导 (Istio-style)

**优先级**: P3 - 未来想法（先记录，待评估）
**状态**: IDEA ONLY — 未设计、未排期。由 conf_sync 明文默认 finding 的讨论衍生（2026-06-21）。
**关联**:
- `docs/superpowers/specs/2026-06-21-conf-sync-plaintext-default-observability-design.md`（本想法的来源讨论）
- `skills/04-review/architecture/fed-proxy-mtls-fail-close.md`（federation 已强制 mTLS，手工证书）
- `edgion-common/src/conf_sync/config.rs`（`ConfSyncSecurityConfig` / `ConfSyncTlsConfig`，当前是手工 cert/key/ca 文件路径）

---

## 0. 一句话目标

让 conf_sync（Controller↔Gateway）和 fed_sync（Controller↔Center）的 mTLS 证书**自动签发与轮转**，
而不是像现在这样要求运维手工准备 `cert` / `key` / `ca` 三个文件路径并自己处理过期轮转。

## 1. 背景：Istio 怎么做的（参照范本）

Istio 的 istiod ↔ Envoy(via istio-agent) xDS 通道（等价于 Edgion 的 conf_sync）默认就是 mTLS，
而且操作者**不需要手工管证书**：

1. 每个 workload 的 istio-agent 拿着 Kubernetes 投影的 **ServiceAccount JWT token**
   （projected SA token，audience = `istio-ca`）。
2. agent 用这个 JWT 向 istiod 内置 **CA** 发 CSR，证明自己的身份。
3. istiod CA 校验 JWT 后签发一张带 **SPIFFE SVID**（`spiffe://<trust-domain>/ns/<ns>/sa/<sa>`）
   的短期 workload 证书。
4. agent 用这张证书和 istiod 建立 **mTLS** 的 xDS 连接，并在到期前自动续签。

要点：mTLS 是默认且零手工证书；身份根来自 K8s SA token；证书短期 + 自动轮转。

## 2. 当前 Edgion 现状（差距）

- conf_sync / fed_sync 的 mTLS 走 `ConfSyncTlsConfig { cert, key, ca, skip_tls }`——**手工文件路径**。
- 证书的来源、签发、轮转全部是运维 / 外部工具（cert-manager、SPIFFE/SPIRE、手工脚本）的责任。
- 没有内置 CA，也没有基于 K8s SA token 的身份引导。

## 3. 可能的方向（待评估，未决策）

- **方向 A — 复用外部 PKI（最轻）**：不自建 CA，标准化到 cert-manager / SPIRE 签发的证书 +
  约定挂载路径，文档化推荐姿势。改动小，但仍依赖外部组件。
- **方向 B — Controller 内置轻量 CA + SA-token CSR（Istio 同款，最重）**：
  Controller 充当 CA，Gateway 启动时用投影 SA token 申请 SPIFFE 证书，自动轮转。
  收益最大（零手工证书、统一身份模型、与 federation SPIFFE 自检对齐），但工程量大、
  需要 CA 生命周期 / 信任根分发 / 轮转 / 撤销一整套。
- **方向 C — 混合**：先 A 落地降低运维门槛，B 作为长期目标。

## 4. 为什么先不做

- 本轮 finding 只需要"明文默认可观测 + 生产推荐 mTLS"，手工证书已能满足 mTLS。
- 自动签发是**降低运维门槛**的增强，不是安全缺口的修复——属于独立的较大工程。
- federation 侧已有 SPIFFE 自检（`peer_identity_self_check`），未来若做内置 CA 应和它统一设计。

## 5. 待办（评估阶段）

- [ ] 调研 Edgion 部署形态是否普遍在 K8s（决定 SA-token 引导是否可行）。
- [ ] 评估直接对接 cert-manager / SPIRE（方向 A）的成本与是否足够。
- [ ] 若考虑内置 CA（方向 B），评估和 federation SPIFFE 模型的统一设计。
- [ ] 决定是否值得做，以及 A/B/C 取舍。
