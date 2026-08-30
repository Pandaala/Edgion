# Edgion Service Mesh (Istio & Linkerd) 集成路线图

为了保持 Edgion 作为高性能 Rust+Pingora 南北向网关的极致专注力，项目放弃了自研服务网格（CA、CNI、L4代理等）的繁重路线，全面转向**“与主流 Service Mesh 强强联合、原生兼容”**的开放生态路线。

本路线图旨在通过零代码改动的方式，使 Edgion 网关完美融入 **Istio** 与 **Linkerd** 网格，为客户提供兼具“高性能入口治理”与“透明微服务加密（mTLS）”的东西向+南北向全套解决方案。

---

## 1. 核心任务清单

本目录下的任务已精简为以下集成与测试验证任务：

1.  **[01-linkerd-integration.md](file:///Volumes/ExtStore/ws4/Edgion/tasks/todo/mesh/01-linkerd-integration.md)**
    *   **内容**：验证并打通 Edgion 融入 Linkerd 网格。包括 Sidecar 注入兼容性验证、联调测试、头部（Header）传播机制、以及在 `edgion-tests` 中编写自动化的端到端 Linkerd 集成测试。
2.  **[02-istio-integration.md](file:///Volumes/ExtStore/ws4/Edgion/tasks/todo/mesh/02-istio-integration.md)**
    *   **内容**：验证并打通 Edgion 对接 Istio 网格的双重模式：
        - **Sidecar 模式**：验证注入 `istio-proxy` 的兼容性与流量转发。
        - **Ambient (Sidecar-less) 模式**：验证 Edgion 作为 Ingress 时，节点级 ztunnel 对 Edgion 出站流量的自动捕获与 HBONE 隧道包装。在 `edgion-tests` 中建立自动化测试用例。

---

## 2. 演进路线（Phase）

```
┌────────────────────────────────────────────────────────┐
│ Phase 1: 验证与集成文档编写                            │
│   - 手动在测试集群部署 Linkerd 与 Istio                 │
│   - 验证 edgion-gateway 注入后的流量可达性             │
│   - 整理并发布用户集成指南手册 (en / zh-CN docs)       │
└───────────────────────────┬────────────────────────────┘
                            │
                            ▼
┌────────────────────────────────────────────────────────┐
│ Phase 2: 自动化集成测试套件开发                        │
│   - 在 edgion-tests 中自动拉起 k3s/kind 测试集群       │
│   - 编写 Linkerd 注入测试套件                          │
│   - 编写 Istio (Sidecar & Ambient) 注入与透明拦截测试  │
└───────────────────────────┬────────────────────────────┘
                            │
                            ▼
┌────────────────────────────────────────────────────────┐
│ Phase 3: 高级优化与发布                                │
│   - 优化网关在网格内的流量头部（Header）透传           │
│   - 验证 Edgion DSL 在网格拦截背景下的性能表现         │
└────────────────────────────────────────────────────────┘
```
