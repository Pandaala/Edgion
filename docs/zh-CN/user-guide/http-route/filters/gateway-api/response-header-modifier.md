# ResponseHeaderModifier

## 概述

`ResponseHeaderModifier` 在响应返回客户端之前修改 HTTP 响应头。它是标准 Gateway API 过滤器，在响应阶段运行，即上游响应头到达之后。使用它可以添加安全响应头、剥离泄露后端信息的响应头，或为响应打上网关元数据标记。

## 响应头修改策略 {#response-header-policy}

Edgion 对 HTTPRoute 和 GRPCRoute 修改器、ResponseRewrite、DSL、Wasm、ExtProc、
配置生成的本地回复（包括 Mock 和 FaultInjection）、ForwardAuth 拒绝响应头、
响应染色及 RegionRoute 写回应用相同策略。名称比较不区分 ASCII 大小写。

| 字段 | 设置、追加、条件写入、删除、重命名 |
|------|----------------------------------|
| 普通字段，如 `Cache-Control`、`WWW-Authenticate`、`Set-Cookie` | 通过功能本身的校验后允许 |
| `Content-Length` | 拒绝 |
| `Connection`、`Keep-Alive`、`Proxy-Authenticate`、`Proxy-Authorization`、`TE`、`Trailer`、`Transfer-Encoding`、`Upgrade`、`Proxy-Connection` | 拒绝 |
| 伪首部，如 `:status` | 不能作为普通响应头修改；使用相应状态码接口 |

重命名的源名称和目标名称都必须允许。策略拒绝发生在任何重命名写入之前。
静态配置在使用前校验，动态名称在应用时检查。此前已成功的操作保留效果，
各扩展保留原有错误处理方式。

限制适用于 HTTP/1.1、HTTP/2、gRPC 响应及配置生成的本地回复；请求头规则独立。
Gateway 仍负责有限本地回复的准确长度、流式响应转换、压缩、gRPC-Web、
仅含 trailers 的错误响应以及 101 升级握手所需的字段；这些功能无需配置分帧头。

转发时会清理收到的 `Connection` 所提名的字段。清理后新写入的同名普通字段
不继承原提名。临时 1xx 响应跳过普通响应插件链，101 保留传输层管理的握手字段。
HEAD 及无响应体状态继续遵守各自协议规则。


## 配置

```yaml
filters:
  - type: ResponseHeaderModifier
    responseHeaderModifier:
      set:
        - name: X-Powered-By
          value: Edgion
      add:
        - name: Cache-Control
          value: no-store
      remove:
        - Server
```

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `set` | Array\<Object\> | 无 | 每条 `{name, value}` 替换该响应头，覆盖所有已有值 |
| `add` | Array\<Object\> | 无 | 每条 `{name, value}` 追加一个值，保留已有值 |
| `remove` | Array\<String\> | 无 | 从响应中删除的响应头名称列表 |

三个操作按顺序应用：`set`，然后 `add`，然后 `remove`。Gateway API 配置要求每个不区分大小写的响应头名称只能执行一种操作，因此同等名称不能在一个列表中重复，也不能跨多个列表使用。

## 示例

### 示例 1：添加安全响应头并隐藏后端信息

添加严格传输安全响应头，并删除上游的 `Server` 标识。

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: api-route
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
        - type: ResponseHeaderModifier
          responseHeaderModifier:
            set:
              - name: Strict-Transport-Security
                value: "max-age=31536000"
            remove:
              - Server
      backendRefs:
        - name: api-service
          port: 8080
```

### 示例 2：多值响应头与 add

`add` 在不移除已有值的情况下追加。如果上游响应包含 `Vary: Accept-Encoding`，以下示例会向客户端返回 `Vary: Accept-Encoding` 和网关追加的 `Vary: Origin`。

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: vary-route
  namespace: production
spec:
  parentRefs:
    - name: main-gateway
  rules:
    - matches:
        - path:
            type: PathPrefix
            value: /
      filters:
        - type: ResponseHeaderModifier
          responseHeaderModifier:
            add:
              - name: Vary
                value: Origin
      backendRefs:
        - name: backend-service
          port: 8080
```

## 注意事项

- 响应头名称不区分大小写。
- 名称必须是 1–256 字节的合法 HTTP 头名称；值必须是 1–4096 个字符的合法 HTTP 头值。CR、LF、NUL 等非法控制字符会被拒绝。
- `set`、`add`、`remove` 各最多包含 16 项。同一个名称（不区分大小写）不能出现在多种操作中。对于通过非 Kubernetes 原生准入入口提供的同列表等价名称，仅使用第一项，但每一项仍必须通过校验。操作顺序为 `set` → `add` → `remove`。
- HTTPRoute 和 GRPCRoute 原生过滤器的 `type` 必须与对应配置字段精确配对。同一个规则或后端过滤器列表内，请求头、响应头修改器分别只能出现一次。规则级与后端级修改器属于不同列表。
- 校验同样适用于 EdgionPlugins 及文件/etcd 配置。无效原生修改器会使所属规则无效，正确的兄弟规则仍可使用，状态会报告配置错误。若非法配置进入 Gateway 构造阶段，对应运行时将以通用配置错误拒绝请求，不再静默跳过修改器。
- 值按字面量处理，不进行变量展开。
- 本过滤器在响应阶段运行；如需修改请求头，请使用 [RequestHeaderModifier](./request-header-modifier.md)。
- 如需重写状态码或进行更丰富的响应转换，请参阅 [ResponseRewrite 插件](../edgion-plugins/response-rewrite.md)。

## 相关文档

- [RequestHeaderModifier](./request-header-modifier.md)
- [ResponseRewrite 插件](../edgion-plugins/response-rewrite.md)
- [插件组合与执行顺序](../plugin-composition.md)
