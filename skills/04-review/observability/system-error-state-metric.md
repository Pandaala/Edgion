# `edgion_system_error_state` gauge：系统级降级状态的登记簿

## 结论

Edgion 使用 `edgion_system_error_state{component, reason}` gauge 登记**"进程运行中但处于某种已知降级/不安全状态"**的情况。典型用例是 `skip_tls=true` 的紧急 TLS 绕过——行为本身保留（operator 有紧急逃生门需求），通过 metric 让 Prometheus/Alertmanager 捕获"降级模式被激活 + 未及时关闭"的情况。

遇到 review 报"Edgion 某处允许不安全模式（`skip_tls`、`AllowInsecureFallback` 等），应在代码层面拒绝启动 / 加两因素确认 / 强制时限"时，按下述规则判定是否复用本机制关闭而非引入新硬约束。

## 设计约束

1. **Metric 名**：`edgion_system_error_state`（gauge，值始终为 `1` 当条件激活；进程重启才会复位）
2. **标签**：
   - `component`：上报主体（定义在 `src/core/common/observe/system_metrics.rs::components`）
     - 当前：`conf_sync_client`、`conf_sync_server`、`fed_sync_client`、`fed_sync_server`
   - `reason`：降级条件类型（定义在 `system_metrics.rs::reasons`）
     - 当前：`skip_tls`
3. **Label 值必须从 bounded 常量中取**，不允许传入自由字符串（Prometheus 基数风险）
4. **设置一次即可**：这些是**启动时确定的配置降级**，不是瞬时故障；不需要清零逻辑（进程 restart 语义已覆盖）
5. **辅配 `tracing::error!`**：日志做启动一次性提示，metric 做持续可观测；两者并存

## 何时用这个机制

**适用**（用 metric 登记 + 保留行为）：
- 有**明确紧急逃生门价值**的降级模式（operator 在极端场景能用）
- 代码层面拒绝会堵死合法用例
- 降级是启动配置决策，非瞬时
- 监控层面能及时发现"忘记关"就足够

**不适用**（应该代码层面拒绝或重构）：
- 降级无任何合法使用场景（纯粹 legacy，删除无损失）
- 降级在运行时频繁切换（应该用 counter / event，不是 state gauge）
- 降级涉及用户可控输入（攻击面更大，不该有逃生门）

## 判定规则：review 说"XX 不安全模式应在代码层禁止"时

按以下顺序判定：

1. **当前是否已上报 metric？**
   - 看 `src/core/common/observe/system_metrics.rs::components` 是否覆盖报告点
   - 看 `system_metrics.rs::reasons` 是否覆盖该降级类型
2. **是否符合"适用"场景**（见上节）？
3. 三点都是 yes → 指向本文件 + `tasks/security-audit/04-admin-api.md` M2 决策关闭
4. 若 review 的点 metric 尚未覆盖 → 评估是否**新增一个 reason 常量**（而非新增独立 metric）；新增后再按上述流程关闭

## 例外场景（需重评）

- 降级模式开始出现 frequent flip（运行时被反复开关）：gauge 语义不适用，需考虑改 counter 或 event log
- `component` / `reason` 基数超过 20 个：Prometheus 开销扩大，需重评 label 是否需要合并或分层
- 某降级模式被证明"完全无合法使用场景"：从 metric 登记升级为代码层面拒绝

## 背景记录

- 决策日期：2026-04-22
- 决策来源：`tasks/security-audit/04-admin-api.md` M2（gRPC `skip_tls=true` 明文连接）
- 实现文件：`src/core/common/observe/system_metrics.rs`
- 首批覆盖站点：5 处 `skip_tls=true` 分支（Gateway / Controller / Center）
- 典型 Alertmanager 规则（operator 侧配置，非 Edgion 内置）:
  ```yaml
  - alert: EdgionSystemErrorState
    expr: edgion_system_error_state > 0
    for: 5m
    annotations:
      summary: "Edgion {{ $labels.component }} running in degraded mode ({{ $labels.reason }})"
  ```
