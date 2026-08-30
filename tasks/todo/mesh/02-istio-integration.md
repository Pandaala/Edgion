# Task 02: Edgion Ingress 与 Istio (Sidecar & Ambient) 网格的对接与测试

## 1. 任务背景

Istio 作为市场份额最大的服务网格，拥有两种数据面模式：经典的 **Sidecar 模式** 与全新的 **Ambient (Sidecar-less) 模式**。
Edgion 作为南北向网关，必须原生兼容这两种模式。对于 Ambient 模式，Edgion 无需注入 Sidecar，直接通过节点级 ztunnel 完成透明 mTLS 融入，这是云原生最先进的网络架构。

---

## 2. 关键验证点与工程任务

### 2.1 Istio Sidecar 模式验证
- 验证在部署 `edgion-gateway` 时，自动注入 `istio-proxy` (Envoy) 的稳定性：
  - 命名空间打标：`istio-injection=enabled`
  - 检查 Envoy 是否能通过 iptables 顺利劫持 `edgion-gateway` 发往后端服务的流量，并升级为 Istio SPIFFE mTLS。
- 检查 `edgion-gateway` 与 Envoy Sidecar 共存时的端口分配，确保 15090 (metrics), 15021 (health check) 等 Istio 保留端口不与 Edgion 冲突。

### 2.2 Istio Ambient (无 Sidecar) 模式验证与透明集成
在 Ambient 模式下，不需要修改 `edgion-gateway` Pod。节点上的 `ztunnel` 会通过 CNI 和 eBPF/iptables 自动接管流量。
- **环境搭建**：安装 Istio 并启用 Ambient 配置文件 (`--set profile=ambient`)。
- **命名空间打标**：给部署 `edgion-gateway` 的命名空间打上 `istio.io/dataplane-mode=ambient` 标签。
- **流量跟踪审计**：
  - 发送外部请求到 `edgion-gateway`。
  - 通过 `tcpdump` 或网络追踪，确认当 `edgion-gateway` 访问后端服务时，流量在物理节点网卡出站前被 `ztunnel` 封装成 **HBONE (HTTP CONNECT over HTTP/2) 隧道加密报文**。
  - 确保整个过程中没有 Sidecar 注入，Edgion 网关维持极致的纯净与高性能。

### 2.3 在 `edgion-tests` 中编写 Istio E2E 测试套件
- 编写自动化脚本：
  1.  利用 `istioctl` 在 Kind/K3s 中自动安装并配置 Istio (分别测试 Sidecar 和 Ambient 两种 Profile)。
  2.  在受控命名空间下部署 `edgion-gateway` 并配置 HTTPRoute 转发。
  3.  模拟流量访问，利用 K8s 内部探针或 Istio 监控工具校验：
      - 南北向流量能正常通过 Edgion 解析路由。
      - 东西向流量 mTLS 状态为加密（无论是 Envoy Sidecar 加密还是节点 ztunnel 隧道加密）。
