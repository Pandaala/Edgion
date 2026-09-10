# RequestMirror

## 概述

`RequestMirror` 将部分入站请求复制到第二个后端，同时由路由的主后端正常处理原始请求。它是标准 Gateway API 过滤器，在请求阶段运行。镜像采用 fire-and-forget 模式：镜像响应被丢弃，永远不会影响客户端，因此可安全地对新服务版本进行生产流量的影子测试。

## 请求体与后端过滤器

可以在 `rules[].filters` 或 `rules[].backendRefs[].filters` 中配置镜像过滤器。
后端级镜像只对选中的后端执行。两个位置都支持携带请求体，无需配置
`RequestBodyBuffer`：主请求向上游流式传输时，在镜像队列和字节预算内复制数据。
如果前序插件已缓存或修改请求体，镜像读取已提交的请求体快照。

单独启用镜像不会禁用主请求重试，也不会改变主请求体的传输 framing。
已经完成或放弃的镜像不会因重试而重新启动；镜像失败时主请求继续处理。

## 跨命名空间授权

镜像目标仅支持核心 Service（`group: ""`、`kind: Service`）。
省略 `backendRef.namespace` 时，使用声明该过滤器的 HTTPRoute 所在命名空间；
通过路由委派继承的过滤器也保留声明者的命名空间。

Controller 默认启用 ReferenceGrant 校验。跨命名空间镜像必须由目标命名空间
中的 ReferenceGrant 授权。例如，允许 `production` 中的 HTTPRoute 镜像到
`testing/mirror-service`：

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: ReferenceGrant
metadata:
  name: allow-production-mirror
  namespace: testing
spec:
  from:
    - group: gateway.networking.k8s.io
      kind: HTTPRoute
      namespace: production
  to:
    - group: ""
      kind: Service
      name: mirror-service
```

没有匹配授权时，路由报告 `ResolvedRefs=False`，原因是 `RefNotPermitted`。
Edgion 跳过被拒绝的镜像，在请求的插件日志中记录 `mirror ref denied`，
主请求继续处理。规则级和后端级镜像过滤器遵循相同规则。
不支持的目标 group 或 kind 会被跳过，并报告 `InvalidKind`。

授权新增、修改和删除会触发重新校验。更新后的配置到达 Gateway 后，
新请求采用新的授权结果；撤销授权不会取消已经启动的镜像任务。

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

过滤器只接受标准 Gateway API 字段：

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `backendRef` | Object | **必填** | 镜像流量的目标后端：`{name, port}` |
| `fraction` | Object | 全量镜像 | 镜像请求的比例：`{numerator, denominator}` |
| `percent` | Integer | 无 | 镜像请求的百分比（0-100）；与 `fraction` 二选一 |

### `fraction` 子字段

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `fraction.numerator` | Integer | 无 | 镜像比例的分子 |
| `fraction.denominator` | Integer | `100` | 镜像比例的分母 |

### 调优注解

镜像调优参数通过 HTTPRoute 的 `metadata.annotations` 中的 `edgion.io/mirror-*`
注解设置，而不是过滤器字段。Gateway API CRD 是结构化 schema，`requestMirror`
内的任何非标准字段都会被 Kubernetes API server 静默剪除；注解则不受影响。
每个注解作用于该路由下的所有 `RequestMirror` 过滤器。

| 注解 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `edgion.io/mirror-connect-timeout-ms` | Integer | `1000` | 镜像请求的连接超时（毫秒） |
| `edgion.io/mirror-write-timeout-ms` | Integer | `1000` | 镜像请求的写入超时（毫秒） |
| `edgion.io/mirror-max-buffered-chunks` | Integer | `5` | 单个镜像最多缓冲的请求体块数，范围 `1..=1024` |
| `edgion.io/mirror-max-buffered-bytes` | String | `1MiB` | 单个镜像最多持有的请求体字节数，范围 `1B..=1GiB` |
| `edgion.io/mirror-log` | Boolean | `true` | 镜像是否生成独立的 access log 条目 |
| `edgion.io/mirror-max-concurrent` | Integer | `1024` | 每个插件实例的最大并发镜像任务数，范围 `1..=65536`，同时受进程共享上限约束 |
| `edgion.io/mirror-channel-full-timeout-ms` | Integer | `0` | 镜像通道满时等待队列空间的时长（毫秒）；`0` 表示立即放弃该次镜像（超过 `1000` 的值会被钳制） |

注解值无效时会记录日志，该字段回退到默认值。

Gateway 进程还通过 `request_mirror.max_concurrent`（默认 `1024`）和
`request_mirror.max_buffered_bytes`（默认 `64MiB`）约束所有镜像实例的总量，
包括热加载期间并存的新旧实例。在
[Gateway 配置文件](../../../../ops-guide/configuration/gateway-config.md#request_mirror)中设置，
重启进程后生效。注解修改则在路由热加载后作用于新请求。预算不足时跳过或取消镜像副本，主请求继续。
字节限制约束镜像持有的请求体数据，不代表累计上传大小或进程 RSS。

对于已缓存的请求体，已提交快照占用进程字节预算；单镜像字节限制约束尚未释放的流式数据块，
不限制快照总长度。磁盘快照仍按块读取。操作者可以调高或调低进程预算，调高字节上限意味着允许
镜像持有更多内存。

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
  annotations:
    edgion.io/mirror-connect-timeout-ms: "1000"
    edgion.io/mirror-write-timeout-ms: "1000"
    edgion.io/mirror-max-buffered-chunks: "5"
    edgion.io/mirror-max-buffered-bytes: "1MiB"
    edgion.io/mirror-max-concurrent: "1024"
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
      backendRefs:
        - name: api-service
          port: 8080
```

## 注意事项

- 镜像是非阻塞的：客户端始终收到主后端的响应，镜像响应被丢弃。
- 若镜像队列已满且 `edgion.io/mirror-channel-full-timeout-ms` 超时，该请求的镜像将被丢弃，而不会延迟原始请求。
- 如果镜像停止前下游传输没有到达真实 EOS，Edgion 会取消镜像请求，避免镜像目标把不完整上传当成完整 body；镜像结果为 `body_abandoned`。只有请求头携带传输层 EOS 才能证明空 body 已完整；HTTP/2 中仅有 `Content-Length: 0` 声明并不能提供该保证。
- 这是标准 Gateway API 镜像，目标是一个 `backendRef`。如需更丰富的镜像选项——例如内联调优字段或超出 `backendRef` 范围的精细控制——请参阅 [Edgion RequestMirror 插件](../edgion-plugins/request-mirror.md)，其配置直接内嵌调优参数（该 CRD 不受结构化 schema 剪除影响）。

## 相关文档

- [Edgion RequestMirror 插件](../edgion-plugins/request-mirror.md)
- [RequestHeaderModifier](./request-header-modifier.md)
- [插件组合与执行顺序](../plugin-composition.md)
