# Task 01: Edgion Ingress 与 Linkerd Mesh 的对接与端到端集成测试

## 1. 任务背景

当应用部署在 Linkerd 服务网格中时，所有的微服务流量均被加密和校验。Edgion 作为南北向 Ingress 网关，必须能够透明地将外部请求导入网格中。
这通过将 Linkerd 的 `linkerd-proxy` 注入到 `edgion-gateway` Pod 中来实现。我们需要进行系统级的兼容性验证并编写自动化的端到端（E2E）测试。

---

## 2. 关键验证点与工程任务

### 2.1 注入兼容性验证 (Sidecar Injection)
- 验证在 `edgion-gateway` 的 Deployment 中添加以下注解后，Linkerd 的 mutating Webhook 能否正确注入 `linkerd-proxy` 容器：
  ```yaml
  linkerd.io/inject: enabled
  ```
- **启动顺序调试**：验证 `linkerd-proxy` 启动后，`edgion-gateway` 进程是否能正常初始化并与其在 localhost 通信，避免端口冲突。
- **特权与安全上下文**：有些 K8s 集群限制了 NET_ADMIN 能力，验证 `linkerd-init`（流量劫持容器）在标准的受限节点上的运行情况。

### 2.2 头部与路由透传 (Header Propagation)
- Linkerd 使用一些特定的控制面头部（例如 `l5d-dst-override`）来实现网格内的某些高级路由。
- 确认 `edgion-gateway` 在向后端发送流量时，能正确透传必要的网格头部，以及分布式追踪头部（如 W3C Trace Context `traceparent`, Zipkin `x-b3-*`）。

### 2.3 在 `edgion-tests` 中编写 E2E 测试套件
在集成测试框架中，编写一个专用的测试用例：
1.  **准备环境**：测试脚本在本地 K3s/Kind 中自动安装 Linkerd CLI 并执行 `linkerd install | kubectl apply -f -` 初始化网格。
2.  **部署网关与服务**：
    - 在打上 `linkerd.io/inject: enabled` 标记的命名空间下部署 `edgion-gateway`。
    - 部署测试应用（`test_server`）并打上注入标记。
3.  **流量测试**：通过外部客户端向 `edgion-gateway` 发起 HTTPS 请求，验证流量能否经过 Edgion 路由，成功经由 Linkerd 自动加密的 mTLS 隧道到达 `test_server`。
4.  **指标校验**：通过 Linkerd CLI 检查 Edgion Pod 的网格状态，确保其被正确识别为 mesh 成员，并且出站流量 mTLS 状态为 `Success`。
