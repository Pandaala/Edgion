# Gateway API Standard CRD 兼容性

Edgion 对 v1.0.0 至 v1.6.2 发布的 **Standard** Gateway API CRD 包运行兼容测试。
Experimental 包不在这项兼容范围内。CRD 包版本（例如 v1.5.0）与资源 API 版本
（例如 `gateway.networking.k8s.io/v1/HTTPRoute`）是两个概念；多个 CRD 包可以
提供同一个资源 API 版本。

## 各版本覆盖的资源

表格中的版本表示对应 Standard 包的 YAML 套件实际提交的资源源版本。`v1 + beta`
表示同时测试 `v1` 与 `v1beta1`，`beta` 仅表示 `v1beta1`。破折号表示该
Standard 包没有提供 Edgion 支持的该资源源版本。每个版本的完整 GVK 清单与 YAML
文件位于 [`edgion-tests/compatible/v*/standard/` 套件](../../../../edgion-tests/compatible/README.md)。

| Standard 包 | GatewayClass | Gateway | HTTPRoute | ReferenceGrant | GRPCRoute | TCPRoute | UDPRoute | TLSRoute | BackendTLSPolicy |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| v1.0.0 | v1 + beta | v1 + beta | v1 + beta | beta | — | — | — | — | — |
| v1.1.0 | v1 + beta | v1 + beta | v1 + beta | beta | v1 | — | — | — | — |
| v1.1.1 | v1 + beta | v1 + beta | v1 + beta | beta | v1 | — | — | — | — |
| v1.2.0 | v1 + beta | v1 + beta | v1 + beta | beta | v1 | — | — | — | — |
| v1.2.1 | v1 + beta | v1 + beta | v1 + beta | beta | v1 | — | — | — | — |
| v1.3.0 | v1 + beta | v1 + beta | v1 + beta | beta | v1 | — | — | — | — |
| v1.4.0 | v1 + beta | v1 + beta | v1 + beta | beta | v1 | — | — | — | v1 |
| v1.4.1 | v1 + beta | v1 + beta | v1 + beta | beta | v1 | — | — | — | v1 |
| v1.5.0 | v1 + beta | v1 + beta | v1 + beta | v1 + beta | v1 | — | — | v1 | v1 |
| v1.5.1 | v1 + beta | v1 + beta | v1 + beta | v1 + beta | v1 | — | — | v1 | v1 |
| v1.6.0 | v1 + beta | v1 + beta | v1 + beta | v1 + beta | v1 | v1 | v1 | v1 | v1 |
| v1.6.1 | v1 + beta | v1 + beta | v1 + beta | v1 + beta | v1 | v1 | v1 | v1 | v1 |
| v1.6.2 | v1 + beta | v1 + beta | v1 + beta | v1 + beta | v1 | v1 | v1 | v1 | v1 |

## 测试范围

`run_compatible_test.sh` 会逐版本安装已固定摘要的官方 Standard CRD，启动
Edgion Controller 和 Gateway，提交相应 YAML，并等待资源的 `Accepted=True`
状态。随后测试 HTTP 路由、ReferenceGrant 授权的跨命名空间访问和 v1beta1
HTTP 路由；对应资源存在时，还会探测 TCP、UDP 和 TLS 透传流量。
GRPCRoute 与 BackendTLSPolicy 只检查协调状态；这套测试不发送 gRPC 流量，
也不验证后端 TLS 校验行为。运行集群测试前，还会按每个 CRD 包的 OpenAPI
schema 检查 YAML。

2026-09-24 在 OrbStack Kubernetes v1.33.9 arm64 集群上，13 个 Standard
版本的兼容测试全部通过。该结果覆盖表中的资源和基本行为，并不等于完整的
Gateway API conformance，也不表示每个可选字段均已实现。使用具体功能前，请
查阅该功能文档和[资源版本边界](../../dev-guide/gateway-api-version-support.md)。

## 运行测试

在一次性集群中运行，并使用集群可拉取的镜像：

```bash
python3 -m pip install -r edgion-tests/gateway_api_bundles/requirements.txt
./edgion-tests/compatible/run_compatible_test.sh \
  --context kind-compatible \
  --controller-image registry.example.com/edgion-controller:current \
  --gateway-image registry.example.com/edgion-gateway:current \
  --test-server-image registry.example.com/edgion-test-server:current
```

`--version v1.5.0` 可只运行一个版本；`--check-artifacts` 可在不连接集群的情况
下验证 CRD 与 YAML schema。集群前提条件和 `--reuse-cluster` 的行为见
[测试脚本说明](../../../../edgion-tests/compatible/README.md)。
