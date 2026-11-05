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
- `edgion.io/dns-resolution-mode` 选择控制面发布的就绪 EndpointSlice（默认，无就绪端点时回退 DNS）、Gateway 缓存 DNS 或每请求 DNS。解析器优先级为 Service、Gateway、EdgionGatewayConfig、系统。
- 一次连接尝试中的主机名、端口和协议来自同一 Service 版本；后续尝试重新读取当前版本。缺少 BackendTLSPolicy 时，即使端口为 443 也保持明文。
- TCP/UDP/TLS 使用已发布的 EndpointSlice，不提供 Gateway 自解析 DNS。

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

## DNS 地址族

DNS 派生的后端地址默认使用 `ipv4Only`。`ipv6Only` 仅使用 IPv6，`all` 使用两种地址族。
只接受这三个精确值；注解为空或非法时拒绝 DNS 请求，不会静默继承或扩大范围。
`all` 不提供 Happy Eyeballs 或连接竞速；IPv6 需要实际可用的上游网络。

Controller 发布优先级为 ExternalName Service 的 `edgion.io/dns-lookup-family`
注解、Controller 的 `dns_resolver.lookup_family`、默认 `ipv4Only`。
Controller 进程配置变更需要重启。Gateway 自解析优先级为 Service 注解、Gateway
同名注解、所选全局 `EdgionGatewayConfig.spec.dnsResolver.lookupFamily`、默认值。
原生 AI 外部域名使用 Gateway、EGC、默认值；资源配置支持热更新。
地址族继承与解析器选择独立，仅设置地址族不会替换 DNS 服务器或 HTTPDNS。

Controller EndpointSlice 由所有消费者共享，Gateway/EGC 默认值不会修改已发布端点，
仅影响自解析及无可用端点时的 DNS 回退。需要两个平面一致时设置 Service 注解；该显式
设置也会过滤尚未清理的 ExternalName 端点，包括 L4 消费者。普通 Service 端点、非 DNS
后端 IP 字面量、监听地址、DNS 服务器地址及 HTTPDNS 自身解析不受此设置影响。
TCP/UDP/TLS 仍需 Controller 发布的 EndpointSlice。

```yaml
metadata:
  annotations:
    edgion.io/dns-lookup-family: ipv6Only
```

Controller 成功获得完整且非空结果后更新所拥有的地址族切片，并删除已消失的另一族。
同一策略下解析失败保留允许的旧地址；收窄策略时即使解析失败也撤下禁止的地址族。
显式使用 Standard DNS 服务器时，空地址族需要携带 SOA 的 NOERROR 否定响应作为证据。
无法确认的空响应和未完成的 CNAME 解析链会保守地按失败处理，避免误删该地址族的已有端点。
Gateway DNS 缓存键包含地址族，热更新不会读取旧策略的缓存答案。
