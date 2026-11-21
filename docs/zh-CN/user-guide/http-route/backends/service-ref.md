# Service 引用

`backendRef` 将 HTTPRoute 规则指向 Kubernetes Service。Gateway（数据面）将该 Service 解析为其活跃端点，并在这些端点之间做负载均衡。

## 基本配置

```yaml
backendRefs:
  - name: api-service
    port: 8080
```

## 配置参考

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `name` | String | **必填** | Service 名称 |
| `port` | Integer | **必填** | Service 端口（Service 暴露的 `port`，非 Pod 的 `targetPort`） |
| `namespace` | String | 路由所在命名空间 | Service 命名空间；跨命名空间需要 ReferenceGrant |
| `kind` | String | `Service` | 原生 Service 引用仅支持省略或精确值 `Service` |
| `group` | String | `""` | API 组；核心 Service 使用 `""` |
| `weight` | Integer | `1` | 相对流量权重，详见 [权重配置](./weight.md) |

## backendRef 如何解析为端点

1. `name`（+ `namespace`）标识对应的 Service。
2. `port` 选择匹配的 Service 端口。Edgion 将其映射到 Pod 侧的 `targetPort` 和对应的 EndpointSlice。
3. Controller（控制面）监视 Service 的 EndpointSlice，并通过 gRPC 将就绪的端点地址推送给 Gateway。
4. Gateway 在这些端点之间对请求进行负载均衡，跳过已被健康检查或熔断器标记为不健康的端点。

端点成员关系来自 EndpointSlice API（而非旧式的单一 `Endpoints` 对象），因此端点集可增量扩展和更新。当 Pod 被添加、移除或就绪状态变化时，更新后的 EndpointSlice 会自动流转到 Gateway，无需重新应用路由。

Service 发生变更（例如端口重命名）时，所有引用该 Service 的路由都会自动触发重处理，保持依赖路由的一致性。

## 默认行为

### kind 字段

- 未指定 `kind` 时，默认为 `Service`。
- API 组必须省略或为空；字面值 `core` 和 `ServiceExternalName` 等伪类型不受支持。
- HTTP/gRPC 根据实际 Service 的 `spec.type: ExternalName` 选择 DNS 行为，引用仍使用 `kind: Service`（或省略），且必须声明有效的 Service 端口。
- `edgion.io/dns-resolution-mode` 决定主机名如何解析（见 [ExternalName 解析](#externalname-解析)）；未设置时使用 EdgionGatewayConfig `dnsResolver.endpointTable` 的全局默认值。
- 一次连接尝试中的主机名、端口和协议来自同一 Service 版本；后续尝试重新读取当前版本。缺少 BackendTLSPolicy 时，即使端口为 443 也保持明文。
- 所有模式都支持 HTTP、gRPC、TCP、UDP 和 TLS 路由。

### namespace 字段

- 省略 `namespace` 时，Gateway 使用 HTTPRoute 自身所在的命名空间。
- 引用不同命名空间的 Service 需要在**目标**命名空间中创建 ReferenceGrant。

### 无法解析的后端

如果引用的 Service 或端口无法解析（名称错误、缺少 ReferenceGrant，或没有就绪的端点），路由的 `ResolvedRefs` 状态会反映故障，对该后端的请求返回 5xx。修复引用后，一旦端点可用，Gateway 会自动恢复。

## 跨命名空间引用

引用其他命名空间的 Service 需要在 **Service 所在命名空间**中创建 ReferenceGrant。若没有该授权，即使 Service 存在，引用也会被拒绝：

```yaml
# production 命名空间中的 HTTPRoute
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: api-route
  namespace: production
spec:
  parentRefs:
    - name: main-gateway
  rules:
    - backendRefs:
        - name: backend-service
          namespace: staging
          port: 8080

---
# 在目标命名空间（staging）中创建的 ReferenceGrant
apiVersion: gateway.networking.k8s.io/v1
kind: ReferenceGrant
metadata:
  name: allow-from-production
  namespace: staging
spec:
  from:
    - group: gateway.networking.k8s.io
      kind: HTTPRoute
      namespace: production
  to:
    - group: ""
      kind: Service
```

## 相关文档

- [权重配置](./weight.md)
- [后端 TLS](./backend-tls.md)
- [后端主动健康检查](./health-check.md)
- [熔断器](./circuit-breaker.md)

## ExternalName 解析

| `edgion.io/dns-resolution-mode` | 行为 |
|---|---|
| `gateway`（默认） | Gateway 在后台解析主机名，把返回的每个地址都变成普通端点，负载均衡、健康检查、异常剔除和会话保持全部生效。请求不等待 DNS；只有主机名尚无解析结果时（Service 刚出现、主机名或解析器刚修改、Gateway 刚启动），每个请求或连接最多等待一次、至多 2 秒。 |
| `system` | Gateway 每个请求（或连接、UDP 会话）都调用操作系统解析器，轮转起始地址拨号一个地址，重试时优先选择尚未尝试过的地址。没有负载均衡池和健康状态。UDP 会经过一个短期共享缓存，监听循环最多等待 250 毫秒。 |
| `controller` | Controller 按固定间隔解析并发布端点，Gateway 只使用这些端点。Kubernetes、文件系统和 etcd 部署都可用。端点只存在于 Controller 内存中，Controller 重启后要等首次解析完成才恢复。 |

未设置注解时由 `EdgionGatewayConfig.spec.dnsResolver.endpointTable` 决定：`true`（默认）
为 `gateway`，`false` 为 `system`。未知取值会让该后端直接失败，而不是另选模式。各模式之间
不会互相兜底。

`gateway` 模式按记录 TTL 刷新（默认 `ttl x 0.5`，限制在 5–300 秒，见
[EdgionGatewayConfig DNS 设置](../../../ops-guide/configuration/edgion-gateway-config.md#后端-dns-端点表)）。
解析结果不变时不做任何更新，健康检查和异常剔除状态得以保留。解析器出错时最多保留上次
的地址 5 分钟；权威的空结果（域名已不存在）会立即撤下端点。操作系统解析器无法区分域名不存在
和故障，因此使用它时，已删除的域名同样会保留这 5 分钟。`169.254.169.254`、`0.0.0.0` 这类链路
本地地址和未指定地址，以及命中 Gateway
[`outbound_egress_blocked_cidrs`](../../../ops-guide/configuration/gateway-config.md#出站封禁-cidr)
的地址，永远不会被使用。Service 删除或主机名变更后，其端点立即撤下；修改解析器
或地址族时，在新解析器返回第一个结果前继续使用上次的结果，只保留新地址族的地址（新解析器失败
则撤下端点）；上次结果中没有新地址族的地址时，请求改为等待首次解析。

解析器优先级：Service 注解 `edgion.io/dns-resolver` / `edgion.io/dns-resolver-ref`，然后
是 EdgionGatewayConfig 默认值（`gateway` 模式）或 Controller 默认值（`controller` 模式），
最后是操作系统解析器。`system` 模式始终使用操作系统解析器。Gateway 资源上的 DNS 注解
不作用于 ExternalName Service。

```yaml
apiVersion: v1
kind: Service
metadata:
  name: payments
  annotations:
    edgion.io/dns-resolution-mode: gateway   # 默认值，可省略
spec:
  type: ExternalName
  externalName: api.payments.example.com
  ports:
    - port: 443
```

## DNS 地址族

DNS 后端地址默认使用 `ipv4Only`；可选 `ipv6Only` 或 `all`，只接受这三个值。空值或未知值
会让 DNS 分发失败，而不是继承或放宽策略。`all` 不启用 Happy Eyeballs 或连接竞速，IPv6
需要可用的上游网络。

ExternalName Service 的 `edgion.io/dns-lookup-family` 注解优先；否则 `gateway` 和 `system`
模式使用 `EdgionGatewayConfig.spec.dnsResolver.lookupFamily`，`controller` 模式使用
Controller 的 `dns_resolver.lookup_family`（Controller 启动时读取）；最终默认 `ipv4Only`。
AI 外部主机名使用 Gateway 注解，然后是 EdgionGatewayConfig 默认值。解析器选择与地址族
继承相互独立，只设置地址族不会替换已选的 DNS 服务器或 HTTPDNS。普通 Service 端点、非 DNS
字面量后端、监听器、DNS 服务器传输和 HTTPDNS 引导连接不受此策略影响。

```yaml
# ExternalName Service metadata
metadata:
  annotations:
    edgion.io/dns-lookup-family: ipv6Only
```

显式使用 Standard DNS 服务器时，空地址族需要携带 SOA 的 NOERROR 否定响应作为证据。
无法确认的空响应和未完成的 CNAME 解析链会保守地按失败处理，避免误删该地址族的已有端点。
