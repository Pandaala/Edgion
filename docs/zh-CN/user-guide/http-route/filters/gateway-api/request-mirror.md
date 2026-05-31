# RequestMirror

## 概述

`RequestMirror` 将部分入站请求复制到第二个后端，同时由路由的主后端正常处理原始请求。它是标准 Gateway API 过滤器，在请求阶段运行。镜像采用 fire-and-forget 模式：镜像响应被丢弃，永远不会影响客户端，因此可安全地对新服务版本进行生产流量的影子测试。

## 配置

```yaml
filters:
  - type: RequestMirror
    requestMirror:
      backendRef:
        name: mirror-service
        port: 8080
      fraction:
        numerator: 10
        denominator: 100
```

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `backendRef` | Object | **必填** | 镜像流量的目标后端：`{name, port}` |
| `fraction` | Object | 全量镜像 | 镜像请求的比例：`{numerator, denominator}` |
| `connectTimeoutMs` | Integer | `1000` | 镜像请求的连接超时（毫秒） |
| `writeTimeoutMs` | Integer | `1000` | 镜像请求的写入超时（毫秒） |
| `maxBufferedChunks` | Integer | `5` | 镜像时缓冲的最大请求体 chunk 数量 |
| `mirrorLog` | Boolean | `true` | 镜像是否生成独立的 access log 条目 |
| `maxConcurrent` | Integer | `1024` | 每个网关进程允许同时飞行的最大镜像任务数 |
| `channelFullTimeoutMs` | Integer | `0` | 镜像通道满时等待队列空间的时长（毫秒）；`0` 表示立即放弃该次镜像 |

### `fraction` 子字段

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `fraction.numerator` | Integer | 无 | 镜像比例的分子 |
| `fraction.denominator` | Integer | `100` | 镜像比例的分母 |

## 示例

### 示例 1：将全部流量镜像到影子服务

将每个请求的副本发送到新版本服务进行对比测试。

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: shadow-route
  namespace: production
spec:
  parentRefs:
    - name: main-gateway
  rules:
    - matches:
        - path:
            type: PathPrefix
            value: /api/
      filters:
        - type: RequestMirror
          requestMirror:
            backendRef:
              name: api-service-v2
              port: 8080
      backendRefs:
        - name: api-service
          port: 8080
```

### 示例 2：镜像 10% 流量并限制资源消耗

只镜像十分之一的流量，并限制并发数和缓冲大小，防止镜像对主路径造成背压。

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: sampled-shadow-route
  namespace: production
spec:
  parentRefs:
    - name: main-gateway
  rules:
    - matches:
        - path:
            type: PathPrefix
            value: /api/
      filters:
        - type: RequestMirror
          requestMirror:
            backendRef:
              name: api-service-v2
              port: 8080
            # 镜像 10% 的请求
            fraction:
              numerator: 10
              denominator: 100
            connectTimeoutMs: 1000
            writeTimeoutMs: 1000
            maxBufferedChunks: 5
            maxConcurrent: 1024
      backendRefs:
        - name: api-service
          port: 8080
```

## 注意事项

- 镜像是非阻塞的：客户端始终收到主后端的响应，镜像响应被丢弃。
- 若镜像队列已满且 `channelFullTimeoutMs` 超时，该请求的镜像将被丢弃，而不会延迟原始请求。
- 这是标准 Gateway API 镜像，目标是一个 `backendRef`。如需更丰富的镜像选项——例如镜像到外部目标或超出 `backendRef` 范围的精细控制——请参阅 [Edgion RequestMirror 插件](../edgion-plugins/request-mirror.md)。

## 相关文档

- [Edgion RequestMirror 插件](../edgion-plugins/request-mirror.md)
- [RequestHeaderModifier](./request-header-modifier.md)
- [插件组合与执行顺序](../plugin-composition.md)
