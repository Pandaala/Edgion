# GrpcJsonTranscoder

通过 HTTPRoute **rule** 的 ExtensionRef 挂载这个请求插件。客户端使用 HTTP/1.1 或 HTTP/2 发送 REST/JSON，Gateway 强制使用 HTTP/2 调用上游原生 gRPC。
路由仍使用既有后端和授权范围；Service 无需 appProtocol 提示。只支持 HTTP/1 的上游返回 502。TLS、后端策略、负载均衡和受支持的目标跳转沿用既有机制。
不允许挂载到 global、Gateway、backendRef 或 GRPCRoute。

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: rest-grpc
  namespace: default
spec:
  requestPlugins:
    - type: GrpcJsonTranscoder
      body:
        maxBodySize: 1m
        onReadFailure: failClose
      config:
        descriptor:
          type: configMapRef
          name: rpc-descriptor
          key: service.pb
        services:
          - example.Greeter
        autoMapping: false
        streamFormat: ndjson
        timeout: 30s
        maxRequestMessageSize: 1m
        maxResponseMessageSize: 512k
        maxResponseJsonSize: 1m
        maxMessageDepth: 32
        maxMessageNodes: 16384
```

将插件挂载到 HTTPRoute rule，并选择 gRPC Service：

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: rest-grpc
  namespace: default
spec:
  parentRefs:
    - name: gateway
  hostnames:
    - api.example.com
  rules:
    - filters:
        - type: ExtensionRef
          extensionRef:
            group: edgion.io
            kind: EdgionPlugins
            name: rest-grpc
      backendRefs:
        - name: greeter
          port: 50051
```



使用 `protoc --include_imports --descriptor_set_out=service.pb` 生成 FileDescriptorSet，保留 google.api.http 选项及其导入描述。
将文件放入同 namespace ConfigMap 的 `binaryData`，也可设置 `descriptor: {type: inline, base64: <encoded bytes>}`。
解码后 descriptor 不得超过 512 KiB。更新和删除会重新编译依赖插件；非法版本关闭流量，包括混合 DSL/WAF 的资源。处理中请求固定使用已选版本。
没有 descriptor URL、文件监视、上传服务、reflection 或运行时 protoc。

HTTP 注解支持 GET/PUT/POST/DELETE/PATCH、路径变量、嵌套字段、additional bindings，以及空、`*` 或字段选择器的 body。
启用 autoMapping 后，无注解方法映射为 `POST /package.Service/Method`，body 为 `*`。
未知 query 字段默认返回 400；`ignoreUnknownQueryParameters: true` 可忽略它们。已知路径/query/body 字段重叠和重复参数仍被拒绝。
ProtoJSON 保留 int64 字符串、枚举名称、bytes base64、map 和受支持的 well-known types。
不支持 Any、HttpBody、group、自定义 HTTP 方法、response_body、客户端/双向流以及压缩消息。

请求体使用 Gateway 共享的 BodyStorage 和物理上限；entry 逻辑上限可进一步收紧 1 MiB JSON 上限。
将转码放在需要 REST 输入的策略之后。随后修改 method/path/query/body 或受保护的 RPC header 会关闭请求。
已有消费者冻结请求体时，转码可能被拒绝；不能与 AI 转换 owner、原生 gRPC、gRPC-web 或 WebSocket 共用。

一元成功响应是一个 ProtoJSON 文档，真实 gRPC status 到达后才提交。
格式错误、截断、超限或缺失 status 返回固定 JSON 502。真实 gRPC 错误映射到 REST 状态，不暴露原始 grpc-message。
当前 Unary Hold replacement 会阻止复用该上游连接。

服务端流每条消息生成一个 record，结束时生成一个明确的 terminal：

```text
{"type":"message","data":{"value":"example"}}
{"type":"end"}
```

错误 terminal 使用 `type: error`，可用时包含数字 grpcStatus。设置 `streamFormat: sse` 后使用 rpc_message、rpc_end、rpc_error 事件。
没有 terminal 的 EOF 表示不完整响应。提交 header 后的失败通过错误 record 或中止连接表达，不再改写 HTTP 状态。

硬上限：每个 protobuf 响应消息 512 KiB，每个 JSON record 和尚未释放的转换输出各 1 MiB，每次回调 256 条消息，深度 32，结构节点 16384。
JSON record 上限至少为 128 字节。进程共享 64 MiB 预算，每个转换预留 32 MiB，最多同时处理两个转换；预算不足返回 503。
取消及最终输出释放会归还容量。插件 `timeout` 默认 `30s`，接受 `1ms` 到 `30s`，例如 `timeout: 5s`；不支持用零关闭。预算从转换计划创建开始，并受已有请求或 gRPC 客户端截止时间限制。热更新仅影响新请求。一元 Hold 使用剩余绝对预算；流式转换在回调检查过期，并将上游 I/O 超时限制到剩余预算。
等待传输事件或下游写入时可能超过回调过期目标。

运行时 protobuf 消息不支持 extension range。非空的显式 string/bytes 默认值在构建反射 descriptor pool 前即被拒绝，自定义 option 的声明也会检查。Map entry 和已知 well-known type 必须具有标准 wire 字段结构。真实 grpc-status 在可修改 trailers 的插件执行前固定，插件不能将失败变为成功。流式输出容量不足会通过既有 sink 取消上游；无法容纳错误 terminal 时，该流是不完整的。

进入反射库前，descriptor 校验会限制 package 为 256 字节、16 段，标识符为
128 个 ASCII 字节，完整名称为 512 字节。文件及导入名称最多 4096 字节，
文件最多 128 个、消息最多 1024 个、字段最多 8192 个；descriptor 的 wire
结构另有深度 32、节点 16384 的限制。超限或非法 descriptor 会使配置校验失败。

Descriptor 冷加载仅支持标准 protobuf options 和已解析的 `google.api.http` 注解。
其他编码后的自定义 options、uninterpreted options、相对 extension extendee 和未知
字段会被拒绝。HTTP 注解使用可信 schema 在进入反射库前检查深度及节点数量，
并验证声明不会替换既有字段类型。ExtProc STREAMED 等可修改流式请求 body 的
处理器不能与转码共用请求；两种插件顺序都会拒绝，即使处理器配置为 fail-open。

一元转换会拒绝最终替换无法执行其正文处理的组合：有效响应正文模式为 STREAMED 的
ExtProc、BandwidthLimit、响应正文阶段 Wasm，以及其他已安装的流式响应处理器。
检查覆盖全局、Gateway、规则和实际选中的 backendRef；两种插件顺序和 fail-open
均不能绕过所有权冲突。仅处理头或 trailer 的兼容插件仍可使用。选中 backendRef
中的 AI 所有者会在插件执行和访问源站之前拒绝。响应头阶段忽略的 DSL 受保护修改
错误也不能发布已准备好的一元响应。WAF 的既有空响应检查窗口保持原有语义。

最终访问日志保留一次转换结果摘要，以及经过验证的 `request_info.grpc_status`。
流已经提交 HTTP 200 后发生的失败也会留下结果；不记录正文或源站 grpc-message。

ProtoJSON 的普通字段 null 表示未设置，包括 optional、oneof 和整个集合；
Value/NullValue 保留其 null 哨兵语义。未知字段和重复别名即使为 null 仍然拒绝。
带引号的整数小数或指数格式使用精确转换；非整数、整数越界和浮点有限数值溢出均拒绝。
NaN、Infinity、-Infinity 使用标准字符串，包括列表或映射中的包装类型。
同一 HTTP 方法的路径模板发生交集或超过有界分析限制时，配置发布失败；不同方法独立。
