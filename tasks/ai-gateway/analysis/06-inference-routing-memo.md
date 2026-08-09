# AI-7 - 推理感知路由

**依赖**: AI-1（provider 抽象，自托管模型作为一种 provider）
**新依赖**: 需要对接推理引擎（vLLM / SGLang）的指标接口
**优先级**: 最低 —— 只有自托管推理集群场景用得上
**对标**: Gateway API Inference Extension（`InferencePool` + Endpoint Picker）,
Envoy AI Gateway 通过它提供该能力

## 问题

代理公有云 LLM 时，网关看不到也管不着推理内部状态，负载均衡只能按延迟/请求数做。
但前面是**自己部署的 vLLM / SGLang 集群**时，普通 LB 算法会显著劣化：

- **KV cache 命中**：同一会话的后续请求打到同一实例，能复用已缓存的 prefix，
  首 token 延迟差异可达数倍。轮询会把这个收益完全抹掉
- **排队深度**：推理实例的"忙"不体现在连接数上，而体现在 GPU 上的 batch 队列。
  按连接数做 LB 会持续把请求塞给一个已经排满的实例
- **LoRA adapter**：多租户共享基座模型时，请求需要路由到已加载对应 adapter 的实例，
  否则触发加载开销

## 上游生态现状

Envoy 自己**没有**把这个能力做进网关本体，而是通过
[Gateway API Inference Extension](https://gateway-api-inference-extension.sigs.k8s.io/)
这个独立项目提供：

- `InferencePool` —— 一组共享基座模型的推理实例
- **Endpoint Picker (EPP)** —— 一个独立的 gRPC 服务，网关把选端点的决策**外包**给它；
  EPP 从各实例拉取指标（KV cache 使用率、等待队列长度、已加载 adapter），做出选择后
  告诉网关打到哪个 pod

这个架构选择值得注意：**选端点的逻辑不在网关里**。原因是它需要持续拉取和聚合推理引擎的
内部指标，这类有状态、有 IO 的逻辑放进数据面热路径不合适 —— 这和 Edgion
`CLAUDE.md` 里"数据面热路径不做 API 调用/定时器"的约束是同一个道理。

## 三条可选路径

### 路径 A：对接现有生态（推荐，如果要做）

实现 Gateway API Inference Extension 的网关侧：支持 `InferencePool` 后端类型，
在选端点时调用外部 EPP 服务。

- 优点：不用自己实现指标采集与调度算法；能直接用生态里已有的 EPP 实现；
  与 Envoy/其它网关的能力对齐
- 缺点：每次请求多一跳 gRPC 调用；需要跟进上游 API 演进
- Edgion 已有的 `ext_proc` 插件是 gRPC 外部处理的现成模式，可以参考其连接管理

### 路径 B：网关内置轻量版

自己拉 vLLM 的 `/metrics`，在网关内维护实例状态，实现简化版的 KV-cache-aware 选择。

- 优点：无额外一跳，延迟最低
- 缺点：指标采集是有状态的后台任务（可以放在配置面，不是热路径，Edgion 的架构允许）；
  但调度算法要自己调优，且要跟着 vLLM 的指标格式变
- 只有在明确要做自托管推理平台产品时才值得

### 路径 C：不做

用 AI-3 的一致性哈希 + 最低延迟算法兜底。会话亲和能拿到一部分 KV cache 收益，
排队深度感知拿不到。

- 对"代理公有云 LLM"为主的场景，这就够了

## 判断

**默认选 C**，除非出现明确的自托管推理平台需求。理由：

1. 这项能力的受众是"自己部署 GPU 推理集群"的用户，是 AI Gateway 用户里的少数
2. 它与推理引擎深度耦合，维护成本随 vLLM/SGLang 演进持续存在
3. Envoy 自己都把它外置成独立项目，说明它不是网关本体该承担的复杂度

如果要做，选路径 A。

## 前置调研（2026-08-03 完成，结论如下）

原有的三个前置问题已由调研回答，见 [../COMPETITIVE-ABSORPTION.md](../COMPETITIVE-ABSORPTION.md)。

### GIE 现状：已 GA，且**范围大幅收窄**（这条改变了成本判断）

- 项目 README 已标 GA，当前发布线 **v1.5.x**（v1.4.0 ≈ 2026-03，v1.5.0 ≈ 2026-04，
  *日期为近似值，仓库只渲染相对时间*）。
- **关键变化**：`InferenceObjective`、`InferenceModelRewrite`、生产版 EPP 与
  Body-Based Router **已全部迁出** `kubernetes-sigs/gateway-api-inference-extension`，
  转到 `llm-d/llm-d-router` 与 `llm-d/llm-d-inference-payload-processor`。
  k8s-sigs 仓库现在只剩 **`InferencePool` API + LWEPP**（用于一致性测试的轻量参考 EPP）。
- 也就是说 **GIE 已经变成很薄的契约，聪明的调度器是别人的问题**。这大幅降低了路径 A 的
  成本：网关侧只需要读 `InferencePool`、挂一个指向 EPP 服务的 ext_proc filter、
  尊重 EPP 返回的端点覆盖——一致性面就这么大。
- **存在性证明**：agentgateway 是 Rust 实现、非 Envoy 的 ext_proc 客户端，
  并宣称是首个通过 GIE v1.4.0 一致性的网关。所以"非 Envoy 代理能否对接"不再是风险。
  已实现方还有 Envoy Gateway、kgateway、GKE Gateway、Istio、NGINX Gateway Fabric。
- 调度信号（由 EPP 侧计算，网关不必自己算）：KV cache 使用率、待处理队列深度、
  已加载 LoRA adapter、请求 criticality、**prefix-cache-aware 打分**。
  v1.5.0 新增可插拔 parser 框架、gRPC response trailers、请求插件镜像到响应路径、
  pool 级 saturation gauge、SLO 驱动的 deadline 排序。

### 判断更新

**默认仍选 C**（不做），但**路径 A 的成本判断要下调**：原文写"需要跟进上游 API 演进"
的顾虑因 GIE 收窄而减轻——现在要跟的只是 `InferencePool` + ext_proc 契约，
而不是整套调度语义。若将来出现自托管推理需求，路径 A 的实现量约等于
"复用现有 `ext_proc` 连接管理 + 一个新后端类型"，比原估计小。

- [ ] 仍然缺的唯一前置：**是否有真实自托管场景需求驱动**，而不是为了能力矩阵完整。
      这一条没被调研回答，也不该由调研回答
