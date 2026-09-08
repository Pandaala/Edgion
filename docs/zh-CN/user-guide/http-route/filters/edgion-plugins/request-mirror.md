# Request Mirror 插件

> **🔌 Edgion 扩展**
> 
> Edgion 同时支持标准 Gateway API `RequestMirror` filter 和可复用的 `EdgionPlugins` `RequestMirror` 配置。
> 本页重点说明可复用的 `EdgionPlugins` 形式，以及两种入口共享的运行时行为。

## 概述

Request Mirror 将入站请求异步镜像到另一个后端服务，主请求的处理不受镜像结果影响。适用于流量复制、灰度测试、请求审计等场景。

**特性**：
- 异步镜像，不阻塞主请求
- 支持按比例采样镜像流量
- 镜像结果可记录在 access log 中
- 支持并发限制，防止镜像目标过载

## 快速开始

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: mirror-plugin
spec:
  requestPlugins:
      type: RequestMirror
      config:
        backendRef:
          name: mirror-service
          port: 8080
```

---

## 配置参数

| 参数 | 类型 | 必填 | 默认值 | 说明 |
|------|------|------|--------|------|
| `backendRef` | Object | ✅ | 无 | 镜像后端引用 |
| `backendRef.name` | String | ✅ | 无 | 后端服务名称 |
| `backendRef.namespace` | String | ❌ | 同路由 | 后端服务命名空间 |
| `backendRef.port` | Integer | ❌ | 无 | 后端服务端口 |
| `fraction` | Object | ❌ | 无 (100%) | 镜像流量比例 |
| `fraction.numerator` | Integer | ✅ | 无 | 分子 |
| `fraction.denominator` | Integer | ✅ | 无 | 分母 |
| `percent` | Integer | ❌ | 无 (100%) | 镜像请求百分比（0-100），Gateway API v1.5 字段名，与 `fraction` 二选一 |
| `connectTimeoutMs` | Integer | ❌ | `1000` | 连接超时毫秒 |
| `writeTimeoutMs` | Integer | ❌ | `1000` | 写入超时毫秒 |
| `maxBufferedChunks` | Integer | ❌ | `5` | 单个镜像最多缓冲的请求体块数，范围 `1..=1024` |
| `maxBufferedBytes` | String | ❌ | `1MiB` | 单个镜像最多持有的请求体字节数，范围 `1B..=1GiB` |
| `mirrorLog` | Boolean | ❌ | `true` | 是否记录镜像日志 |
| `maxConcurrent` | Integer | ❌ | `1024` | 每个插件实例的最大并发镜像数，范围 `1..=65536`，同时受进程共享上限约束 |
| `channelFullTimeoutMs` | Integer | ❌ | `0` | 通道满时等待超时毫秒 |

EdgionPlugins 会将非法容量值（包括零）报告为配置错误。对应的 HTTPRoute
注解则在加载配置时记录非法值，并使用该字段的默认值。

所有实例还共享 Gateway 进程预算：`request_mirror.max_concurrent`（默认 `1024`）
和 `request_mirror.max_buffered_bytes`（默认 `64MiB`）。在
[Gateway 配置文件](../../../../ops-guide/configuration/gateway-config.md#request_mirror)中设置，
重启进程后生效；路由或插件参数则在配置热加载后作用于新请求。预算不足时跳过或取消镜像副本，
主请求继续。字节预算约束镜像持有的请求体数据，不代表进程 RSS，也不包含连接和 TLS 开销。

对于已经缓存的请求，镜像使用独立游标读取已提交的请求体快照，其持有的内存计入进程字节预算。
`maxBufferedBytes` 限制尚未释放的流式数据块，不限制快照总长度；磁盘快照仍按块读取。
操作者可以调高或调低进程预算，调高字节上限意味着允许镜像持有更多内存。

---

## 常见配置场景

### 场景 1：全量镜像到测试服务

```yaml
requestPlugins:
  - type: RequestMirror
    config:
      backendRef:
        name: test-service
        namespace: testing
        port: 8080
      mirrorLog: true
```

### 场景 2：按比例采样镜像

只镜像 10% 的流量：

```yaml
requestPlugins:
  - type: RequestMirror
    config:
      backendRef:
        name: analytics-service
        port: 8080
      fraction:
        numerator: 1
        denominator: 10
```

### 场景 3：限制并发防止过载

```yaml
requestPlugins:
  - type: RequestMirror
    config:
      backendRef:
        name: mirror-service
        port: 8080
      maxConcurrent: 50
      connectTimeoutMs: 500
      writeTimeoutMs: 500
```

---

## 注意事项

1. 镜像发送是异步执行的，运行时资源不足不会改变主请求的响应状态；复制已获预算的数据会增加有界处理开销，显式启用队列等待也会增加延迟
2. 镜像失败不会导致主请求失败
3. 请求体也会被镜像，`maxBufferedChunks` 和 `maxBufferedBytes` 分别限制持有的块数和字节数；它们限制同时持有的数据，而非累计上传大小
4. 当并发镜像数达到 `maxConcurrent` 上限时，新的镜像请求会被丢弃
5. 如果镜像停止前下游传输没有到达真实 EOS，Edgion 会取消镜像请求，避免镜像目标把不完整上传当成完整 body；镜像结果为 `body_abandoned`。这既包括主上游停止消费请求 body，也包括后置请求阶段插件在 body pump 运行前结束请求。只有请求头携带传输层 EOS 才能证明空 body 已完整；HTTP/2 中仅有 `Content-Length: 0` 声明并不能提供该保证

---

## 相关文档

- [ProxyRewrite](./proxy-rewrite.md)
- [过滤器总览](../overview.md)
