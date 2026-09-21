# 第一个 Gateway

本页给你一个最小可理解的对象组合，从运行中的 Edgion 到一条可用的 HTTP 请求：

1. `GatewayClass`
2. `Gateway`
3. `HTTPRoute`

目标不是覆盖所有细节，而是端到端打通一条请求路径，并用 `curl` 验证。

本文假设使用 **Kubernetes 模式**，且已有一个带运行中 Pod 的后端 Service。如果使用 Standalone 模式，请参考 [安装部署](./installation-and-deployment.md) 了解相同资源如何放置在文件系统配置目录中。

## Step 0：安装 Edgion

部署 Controller（控制平面）、Gateway（数据平面）、CRD 及基础配置：

```bash
cicd/deploy/kubernetes/scripts/deploy.sh -y
```

确认控制平面和数据平面正常运行：

```bash
kubectl -n edgion-system get pods
```

这一步确立了一个运行中的 Controller（读取资源）和 Gateway（处理流量）。更多内容见：[安装部署](./installation-and-deployment.md)。

## Step 1：GatewayClass

`GatewayClass` 决定由哪个控制器来管理 Gateway。

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: GatewayClass
metadata:
  name: public-gateway
spec:
  controllerName: edgion.io/gateway-controller/edgion-system/edgion-controller
```

> `GatewayClass` 是集群级资源，没有 `namespace` 字段。

这一步确定了：

- 由哪个控制器（`controllerName`）来管理后续创建的 Gateway
- 后续的 Gateway 通过 `gatewayClassName` 引用这个 GatewayClass

`deploy.sh` 的基础配置已经创建了 `public-gateway` GatewayClass，如果你使用了 Step 0，可以跳过此步。如果后续需要 Edgion 的网关级高级配置，可以加上 `parametersRef`。更多内容见：[GatewayClass 配置](../ops-guide/gateway/gateway-class.md)。

## Step 2：Gateway

`Gateway` 定义流量入口，也就是 listener。

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: Gateway
metadata:
  name: main-gateway
  namespace: edgion-test
spec:
  gatewayClassName: public-gateway
  listeners:
    - name: http
      protocol: HTTP
      port: 80
      allowedRoutes:
        namespaces:
          from: All
```

这一步确定了：

- 用哪个 `GatewayClass`
- 暴露哪些协议和端口
- 允许哪些命名空间的路由绑定到这个 listener

更多内容见：[Gateway 总览](../ops-guide/gateway/overview.md)、[HTTP 监听器](../ops-guide/gateway/listeners/http.md)。

## Step 3：后端 Service

将路由指向一个真实的后端。任何 HTTP Service 均可；以下示例部署一个小型 echo 服务器：

```yaml
apiVersion: apps/v1
kind: Deployment
metadata:
  name: echo
  namespace: edgion-test
spec:
  replicas: 1
  selector:
    matchLabels:
      app: echo
  template:
    metadata:
      labels:
        app: echo
    spec:
      containers:
        - name: echo
          image: ealen/echo-server:latest
          ports:
            - containerPort: 80
---
apiVersion: v1
kind: Service
metadata:
  name: echo-service
  namespace: edgion-test
spec:
  selector:
    app: echo
  ports:
    - port: 8080
      targetPort: 80
```

这一步确立了一个 Gateway 可以转发流量的后端。更多内容见：[Service 引用](../user-guide/http-route/backends/service-ref.md)。

## Step 4：HTTPRoute

`HTTPRoute` 把请求绑定到 Gateway listener，并把流量转发给后端服务。

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: example-route
  namespace: edgion-test
spec:
  parentRefs:
    - name: main-gateway
      sectionName: http
  hostnames:
    - "example.com"
  rules:
    - matches:
        - path:
            type: PathPrefix
            value: /
      backendRefs:
        - name: echo-service
          port: 8080
```

这里最关键的是：

- `parentRefs` 把 route 绑定到 `Gateway`
- `sectionName` 对应 listener 名称（`http`）
- `backendRefs` 指向真正接收流量的服务和端口

更多细节见：[HTTPRoute 总览](../user-guide/http-route/overview.md)。

## Step 5：应用并验证

应用所有资源（假设你已将上面的 YAML 保存到文件中，或直接内联应用）：

```bash
kubectl create namespace edgion-test --dry-run=client -o yaml | kubectl apply -f -
kubectl apply -f gateway.yaml -f backend.yaml -f httproute.yaml
```

检查路由已附加、Gateway 已配置完成：

```bash
kubectl -n edgion-test get gateway main-gateway
kubectl -n edgion-test get httproute example-route
```

通过 Gateway 发送请求。快速本地验证可以将 Gateway Service 端口转发到本机。先找到 Service 名称（在 `edgion-system` 命名空间）：

```bash
kubectl -n edgion-system get svc
kubectl -n edgion-system port-forward svc/<gateway-service> 8080:80
```

在另一个终端，携带匹配的 `Host` 头发送请求：

```bash
curl -H "Host: example.com" http://127.0.0.1:8080/
```

你应该能看到 echo 服务器的响应，确认请求已经流经 `Gateway`、匹配 `HTTPRoute`，并到达 `echo-service`。

## Step 6：理解这条流量

这几个对象串起来以后，请求路径大致是：

1. 客户端请求打到 `Gateway` 的 listener
2. `HTTPRoute` 按 hostname、path、headers 等规则匹配
3. 选中 `backendRefs`
4. Gateway 把请求转发到后端服务

如果后面要继续加能力，通常是在 `HTTPRoute` 上继续叠加：

- 匹配规则
- 标准 Gateway API filters
- Edgion 扩展插件
- 重试、超时、会话保持

## 下一步建议

如果你只是要先会用，继续读：

- [用户指南 / HTTPRoute 总览](../user-guide/http-route/overview.md)
- [用户指南 / 过滤器总览](../user-guide/http-route/filters/overview.md)
- [用户指南 / 后端配置](../user-guide/http-route/backends/README.md)

如果你还没完全理解对象关系，先回去读：

- [核心概念](./core-concepts.md)
