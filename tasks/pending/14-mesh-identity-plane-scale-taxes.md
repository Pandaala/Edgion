# 14 - Service-Mesh Identity-Plane "Scale Taxes" (deferred from conf_sync identity-plane v0)

**优先级**: P3 - 未来想法（mesh 真要给应用 workload 发身份那天才做）
**状态**: DEFERRED — 有意推迟，不在 conf_sync identity-plane v0 范围内。源自 2026-06-21 的 #13 brainstorm。
**关联**:
- `tasks/pending/13-conf-sync-spiffe-cert-bootstrap.md`（本想法的源任务）
- `docs/superpowers/specs/2026-06-2x-conf-sync-identity-plane-v0-design.md`（v0 设计，建成 2 块地基：内置 CA + 统一 SPIFFE 身份；待写）
- `skills/04-review/architecture/fed-proxy-mtls-fail-close.md`（federation mTLS 现状）

---

## 0. 一句话

Istio 式工作负载 mTLS 是 6 块：**内置 CA + 统一 SPIFFE 身份 + TokenReview 防伪造 + CSR-over-gRPC + 信任根分发 + SDS 热交付 + 轮转循环**。
其中**内置 CA** 与**统一 SPIFFE 身份**是 mesh 地基，已纳入 conf_sync identity-plane v0；
本文件记录**被有意推迟的 4 块"规模税"**——它们只在"给互不信任的应用 workload 大规模发身份"这个特定难题下才需要，
conf_sync 这条 2 端点自家链路没有该难题，故不预建。

## 1. 为什么推迟（判定依据）

这 4 块解决的是 Istio 特有的"零信任 @ 规模"问题（成千上万互不信任、动态生灭的 workload），
不是 conf_sync 的痛点。预先为"还没承诺落地的 mesh"建它们 = 过度工程（项目反感无谓 churn）。
它们与 v0 的 CA/身份地基**正交**：v0 建不建不影响以后追加，追加时也不需要推倒 v0。

## 2. 推迟项清单（每块：用途 + 触发条件）

### 2.1 TokenReview 防伪造身份验证
- **用途**：签证书前用 K8s TokenReview 验证请求方的 projected SA token，确认它真是 ns X / sa Y，
  防止恶意 pod 申请写着别人身份的证书去冒充。CA 用验过的 token 身份**强制覆写** SAN（请求里的 SAN 不算数）。
- **v0 不需要的原因**：conf_sync 两端都是 Edgion 自家可信组件，同一信任边界，无"敌对第三方冒充"威胁模型。
- **触发条件**：mesh 开始给**应用 workload**（非 Edgion 自家组件）签发身份证书时。
- **依赖**：CA 改成受 CSR 流程驱动（见 2.2）；Controller 需新增 projected-token 处理 + TokenReview 调用
  （今天只读 SA 的 namespace 文件，无 token 处理）。

### 2.2 CSR-over-gRPC（远程签发协议，私钥不离开 workload）
- **用途**：workload 本地生成 key、只把 CSR 送给 CA，换回签好的证书——私钥永不离开 pod；
  这条 gRPC 通道顺带承载 2.1 的 token 和返回证书链。
- **v0 不需要的原因**：v0 用**中心签发**——Controller 生成两端 key+cert，Gateway 的那张走现成 Secret 轨道送达
  （两端都是自家组件时，中心生成 Gateway key 可接受）。
- **触发条件**：要求"workload 私钥绝不离开 workload"，或 workload 数量/动态性使中心签发不再现实。
- **依赖**：v0 已把 CA 抽成 `CertIssuer` 接口——这里加一个"受理 CSR"的实现即可，不必推倒 v0。

### 2.3 信任根分发机制（ClusterTrustBundle / configmap 注入）
- **用途**：把 root CA 证书自动发到每个 workload（Istio 用 configmap 注入，K8s 1.35+ 有原生
  `ClusterTrustBundle`），让动态生灭的 workload 都能验对端。
- **v0 不需要的原因**：conf_sync 只有 2 端点，root（`ca.crt`）已经在自动签发写入的两个 Secret 里，
  "分发"就是配置里的 `ca` 字段，无需机制。
- **触发条件**：参与方变成"大量动态 namespace/pod"，无法逐个挂 root 时。
- **可选实现**：评估直接用 K8s 原生 `ClusterTrustBundle`（v1.35 beta / 1.36 v1beta1），而非自建 configmap 注入。

### 2.4 SDS 式热交付（轮转不重启数据面）
- **用途**：通过 pod 内 Unix socket 把新证书喂给长跑数据面代理，**不重启**；UDS 让 key 不落盘不上网。
- **v0 是否需要取决于一个待定决策**：conf_sync identity-plane v0 里"轮转能否接受 Gateway 短暂重连"。
  - 若 v0 接受重连/重启 → SDS 热交付推迟到这里。
  - 若 v0 要求热更 → 部分热交付能力会被拉回 v0（按 v0 设计定）。
- **触发条件**：mesh sidecar（必须热更、不能重启）出现，或 conf_sync 证书寿命缩短到重连频率不可接受。
- **关注**：K8s 原生 `PodCertificateRequest`（KEP-4317，v1.35 beta）提供 kubelet 自动注入+轮转的 projected
  pod 证书——评估它能否替代自建 SDS 热交付（注意其内置 signer 只签"认证到 apiserver"的证书，
  自有信任域仍需自带 signer 控制器）。

## 3. 重新评估触发器（总）

当以下任一为真时，从本文件挑相应块落地：
- mesh 开始给**应用 workload**（非 Edgion 自家组件）发 mTLS 身份 → 2.1 + 2.2 + 2.3。
- 出现"必须热更不能重启"的数据面证书消费者 → 2.4。
- 合规/安全要求"私钥绝不离开 workload"或"证书寿命 < 重连可容忍频率" → 2.2 / 2.4。

## 2.5 v0 评审后追加推迟的项（2026-06-21，4 轮 opus review 结论）

conf_sync v0 最终收敛为「最小 Shape A′」——纯自动证书签发+轮转，无身份强制、无投机结构。
以下原本考虑放进 v0、但被评审判为「投机/过度建造/形状未知」而推迟：

- **`CertIssuer` trait（抽象签发后端）**：v0 只有一个 rcgen 自签实现，trait = 单实现投机；
  且未来后端（cert-manager / KEP-4317）是声明式/异步，形状不同，现在抽的 trait 八成是错的。
  → v0 写**具体 struct**（对齐 ACME 先例，ACME 也没躲在 trait 后面）；第二个后端到货再抽 trait。
- **SPIFFE 解析器搬到 `edgion-common`**：v0 不做任何 SPIFFE 校验，解析器零调用者；强行泛化
  现有 `fed_sync/spiffe.rs`（写死 `controllers/<cluster>/<name>`）正是 bug 温床。
  → 解析器留在 `fed_sync`；enforcement 切片真需要时再搬+泛化。
- **在证书里嵌入 SPIFFE SAN**：v0 是共享身份，嵌进去的值未来 per-instance 切片立刻会变；
  自动轮转让「以后再加 SAN」几乎免费。→ v0 **只在 spec 文档里钉死命名语法**，证书不嵌 SAN。
- **fail-close 翻转**（省略证书 = 拒绝启动）：auto-CA 保证证书存在后，conf_sync 可从
  `plaintext_default` 可观测翻成 fail-close（上轮明文 finding 文档明写的 #13 重评触发器）。
  → **作为独立 fast-follow**，等 auto-CA 现场验证稳了再做（带迁移说明 + 退退 plaintext_default 指标）。
- **运维提供 root（operator-provided root 功能）**：v0 仅自签。注意 v0 的「幂等 adopt 已存在的 CA Secret」
  已经免费提供了「自带 root」的非正式逃生口（运维预先创建 CA Secret 即被采纳），正式功能推迟。
- **根证书轮转**：v0 长寿命根（多年）+ 只轮转叶子。根轮转需要交叉签名重叠窗口，否则切换时全断——
  整块推迟，v0 只记录根 notAfter 并在临近到期告警。

触发条件同 §3：当 mesh 真要给应用 workload 发身份、或出现「私钥绝不离开 / 热更不重启 / 自带 PKI」
等具体诉求时，从本文件挑相应块落地。

## 4. 不需要做的明确记录

- 不要因为"以后可能做 mesh"就在 conf_sync v0 里预建以上任何一块。
- v0 在 spec 文档里钉死了 SPIFFE 命名语法（北极星），但**不**在代码里预建 trait/解析器/SAN；
  落地各块时对准**已知形状**新增，不改 v0 既有路径。
