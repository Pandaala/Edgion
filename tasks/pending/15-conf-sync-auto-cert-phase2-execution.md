# 15 - conf_sync Auto-Cert Provisioning v0 — Phase 2 (Rotation) EXECUTION

**优先级**: P2 - Phase 1 集群验证通过后执行
**状态**: PLAN READY, NOT STARTED — plan 已写好并自检通过，**等 Phase 1 集群验证通过再执行**。
**前置依赖**: **Phase 1 必须先在真集群（OrbStack/kind）验证通过**（见下方验证清单）。Phase 2 的轮转建在 Phase 1 的 provisioning 之上；Phase 1 若有 cluster-only 坑，Phase 2 是建在沙子上。

**关联**:
- Plan: `docs/superpowers/plans/2026-06-21-conf-sync-auto-cert-provisioning-v0-phase2-rotation.md`
- Phase 1 plan: `docs/superpowers/plans/2026-06-21-conf-sync-auto-cert-provisioning-v0-phase1.md`（已实现，未 commit，未集群验证）
- Spec: `docs/superpowers/specs/2026-06-21-conf-sync-auto-cert-provisioning-v0-design.md` §5.6

---

## 0. 一句话

执行 Phase 2 rotation plan：让 conf_sync mTLS 叶子证书在 50% 寿命自动轮转，不重启 Gateway/Controller 进程。

## 1. Phase 2 的 4 个 task（详见 plan）

1. `cert_util`：notAfter 解析 + rotation-due 数学 + validate-before-swap（纯函数，单测可验）。
2. leader-gate 的 leaf 轮转 reconciler（仿 ACME 调度器；50% 重签 leaf Secret + 重 mint server 身份到 ArcSwap `ServerIdentityHandle`）。
3. **Controller conf_sync 监听器重绑**（rotation Notify 驱动的 serve 监督循环）—— ⚠️ **最高风险**的并发改动，必须集群验证。
4. Gateway leaf file-watch + validate-before-swap + 重建 Channel 后 `set_all_grpc_clients` 重扇出。

## 2. 为什么先不执行（工程判断，2026-06-21）

- Phase 1 核心（provision/冷启动/RBAC）是 cluster-only 才能验，最终 review 已抓到 2 个 Critical（SAN 缺 -leader、SSA 不 409 的 split-brain），都已修但**未在集群跑过**。
- Task 3 监听器重绑是真·高风险并发代码，盲写易出微妙错误，最好有"编译器+集群"反复迭代。
- subagent 访问当时间歇性被封。

## 3. 执行前提：Phase 1 集群验证清单（必须先全过）

在 OrbStack/kind 上 `kubectl apply -f cicd/deploy/kubernetes/`（先编镜像/推镜像），然后核对：

- [ ] Controller pod 起得来（无证书 Secret 也不卡 ContainerCreating —— 验证 `optional` 卷 + 内存 server 证书）。
- [ ] Controller 自动创建了 `edgion-conf-sync-ca` Secret（含 ca.crt + ca.key）。
- [ ] Controller 自动创建了 `edgion-conf-sync-certs` Secret（含 client.crt/client.key/ca.crt，**无 ca.key**）。
- [ ] Gateway pod 起得来（卷 `optional: true`，Secret 还没好时不卡）。
- [ ] Gateway 经自动 mTLS 连上 Controller（看 gateway 日志 `conf_sync mTLS enabled ... client`，controller 日志 `auto-CA, in-memory server cert`；无 `plaintext_default` 指标）。
- [ ] **双副本 split-brain 验证**：controller scale 到 2，确认两副本收敛到同一个 root（`edgion-conf-sync-ca` 只有一个 ca.crt，重启任一副本仍 mTLS 正常）。
- [ ] **SAN 验证**：gateway 对 `edgion-controller-leader.<ns>.svc.cluster.local` 的主机名校验通过（C1 修复）。
- [ ] **adopt 验证**：预先手动建 `edgion-conf-sync-ca`（自带 root），确认 controller 采纳而非覆盖。
- [ ] RBAC：controller 能 create/patch Secret；gateway SA 读不到 `edgion-conf-sync-ca`。

任一不过 → 先修 Phase 1，再执行 Phase 2。

## 4. 执行方式

Phase 1 验证全过后，用 subagent-driven（或 inline）执行 Phase 2 plan，代码留本地不 commit（沿用本轮约定）。Task 3 边写边在集群上验监听器重绑 + gateway 重连无数据面掉链。
