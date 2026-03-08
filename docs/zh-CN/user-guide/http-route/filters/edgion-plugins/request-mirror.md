# Request Mirror 插件

> **🔌 Edgion 扩展**
> 
> RequestMirror 是 `EdgionPlugins` CRD 提供的请求镜像插件，不属于标准 Gateway API。

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
    - enable: true
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
| `connectTimeoutMs` | Integer | ❌ | `1000` | 连接超时毫秒 |
| `writeTimeoutMs` | Integer | ❌ | `1000` | 写入超时毫秒 |
| `maxBufferedChunks` | Integer | ❌ | `5` | 最大缓冲 chunk 数 |
| `mirrorLog` | Boolean | ❌ | `true` | 是否记录镜像日志 |
| `maxConcurrent` | Integer | ❌ | `1024` | 最大并发镜像数 |
| `channelFullTimeoutMs` | Integer | ❌ | `0` | 通道满时等待超时毫秒 |

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

1. 镜像是异步执行的，不会影响主请求的延迟和状态码
2. 镜像失败不会导致主请求失败
3. 请求体（body）也会被镜像，`maxBufferedChunks` 控制缓冲大小
4. 当并发镜像数达到 `maxConcurrent` 上限时，新的镜像请求会被丢弃

---

## 相关文档

- [ProxyRewrite](./proxy-rewrite.md)
- [过滤器总览](../overview.md)
