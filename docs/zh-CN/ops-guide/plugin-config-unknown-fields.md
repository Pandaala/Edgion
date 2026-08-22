# 未知配置字段检测

`EdgionPlugins`、`EdgionStreamPlugins`，以及 `EdgionConfigData` 的类型化变体，都把配置放在
CRD 中保留未知字段的节点下。其结果是：Kubernetes API server 不会裁剪掉这些 `config` 块中拼写错误的
键，Controller 的 Rust 反序列化就成了唯一还存在的关口。如果没有专门的检查，无法识别的键会
被静默丢弃：`deny:` 被误写成 `denys:`，会让某个 `IpRestriction` 条目的 deny 列表变成空、
`Accepted: True`，而任何地方都不会提示这条规则从未被加载。对于安全相关的插件来说，这是
fail-open（失败即放行），而不是无害的空操作。

对于 `EdgionConfigData`，检测器覆盖 `KeyList`、`IpList`、`Selector` 和
`RegionRouteOverride` 类型化 payload。`data.type: Misc` 是文档明确声明的自由格式逃生口，因此
`Misc.config` 下的任意键不会被报告。Kubernetes 会在 Controller 看到对象之前裁剪结构化
`spec` 和 `data` 层的未知 sibling；FileSystem、etcd、Admin 写入及 validate 这些能读取原始
文档的入口，还可以报告这些 envelope 键。

本文档是这个检测器的运维指南。

## 明确表态

**我们只警告，不阻断。阻断畸形配置应该发生在 apply 之前。**

Controller 永远不会因为出现无法识别的配置键而拒绝一个资源。`Accepted` 保持不变，配置仍然
会按照没有这个功能时一样同步给 Gateway，拼写错误的键依然会被忽略——唯一的变化是现在有人会
被告知。这里没有严格/拒绝模式，也不打算引入：拒绝会带来这个功能刻意没有回答的一系列问题
（一个在拒绝模式开启前就已经 apply 并在同步的资源该怎么处理、Controller 重启时整个存储被
重新摄入该怎么处理、每种 conf_center 模式在拒绝没有运维人员可以上报时应该怎么办）。如果你
想要阻断，请在下面的 `POST /api/v1/validate/{kind}` 之上自己搭建——比如
`ValidatingAdmissionWebhook`、CI 检查，或 GitOps 的 pre-merge 步骤，这些都发生在 Controller
看到该对象之前。

## 发现结果出现在哪里

运维人员可以在四个地方看到发现的问题：

1. **一条 `warn!` 日志**，在资源被 apply 的那一刻由 Controller 打出，携带该资源的
   namespace/name 以及有界的无法识别键路径摘要：最多 20 条具体路径并附带省略标记，且每条
   路径最多 512 字节。
2. **资源自身的 `UnknownConfigFields` 状态条件**：

   ```yaml
   status:
     conditions:
       - type: UnknownConfigFields
         status: "True"
         reason: UnknownFieldsDetected
         message: "requestPlugins[0].config.denys"
   ```

   `kubectl describe` 会像展示其他条件一样展示它。它**不会**影响 `Accepted`（其含义仍然是
   "资源有效且已同步"），也**不会**计入 `edgion_controller_resource_abnormal` 指标——运维
   人员的拼写错误不属于资源健康异常。修正键之后，下一次 apply 会自动移除这个条件；它不会
   停留在过期的 `True` 状态。

3. **`GET /api/v1/diagnostics/unknown-fields`**——覆盖 Controller 当前持有的每一个
   `EdgionPlugins`、`EdgionStreamPlugins` 和 `EdgionConfigData` 资源的全局汇总视图：

   ```json
   {
     "success": true,
     "data": {
       "total": 2,
       "truncated": false,
       "items": [
         {
           "kind": "EdgionPlugins",
           "namespace": "default",
           "name": "auth-chain",
           "paths": ["requestPlugins[0].config.denys"]
         }
       ]
     }
   }
   ```

   需要标准的 Admin API bearer token。`total` 是在 100 条响应上限之前的真实计数；触及上限
   时 `truncated: true`。

4. **Admin API 写入响应中的 `warnings`**——当你通过 Admin API `POST`/`PUT` 一个
   `EdgionPlugins`、`EdgionStreamPlugins` 或 `EdgionConfigData` 文档，且其中含有无法识别的键时，响应会立即携带
   它：

   ```json
   {
     "success": true,
     "data": "EdgionPlugins created",
     "warnings": ["requestPlugins[0].config.denys"]
   }
   ```

   这是唯一没有"第二次机会"在别处上报发现的入口：Admin API 写入路径会先做一次类型化的
   round-trip 再持久化，这会在文档写入存储**之前**就把无法识别的键剥离掉。文件系统 loader、
   etcd loader，以及 Kubernetes watcher 都会在各自的入口上捕获同一类发现，但如果你是通过
   Admin API 写入的，这条响应就是唯一能看到发现结果的地方。写入本身依然会成功
   （`success: true`）——这与上面"只警告、不阻断"的立场一致，属于提示性信息。

## 搭建自己的阻断关卡

`POST /api/v1/validate/{kind}` 提供了针对一份文档（按你实际将要 apply 的样子）的同步、无副
作用的判定结果——不会持久化任何东西，也不会推进 resourceVersion：

```
POST /api/v1/validate/EdgionPlugins
<文档内容，YAML 或 JSON，与实际 apply 时一致>

{
  "success": true,
  "data": {
    "ok": false,
    "unknownFieldCheck": "performed",
    "unknownFields": ["requestPlugins[0].config.denys"],
    "schemaErrors": [],
    "validationErrors": []
  }
}
```

`unknownFieldCheck` 对 `EdgionPlugins`、`EdgionStreamPlugins` 和 `EdgionConfigData` 是
`"performed"`，其余所有类型都是 `"unsupported"`。请务必检查
这个字段，不要想当然地认为空的 `unknownFields` 就意味着"检查过、没发现问题"：对于
`unsupported` 的类型，它的含义是"根本没有检查"。把这个接口接入你想用来实现阻断的任何关卡
——`ValidatingAdmissionWebhook`、GitOps 流水线里的 CI 步骤，或是你自己工具里
`kubectl apply` 之前的检查。

## 相关文档

- [EdgionPlugins —— HTTP 插件](https://github.com/Pandaala/Edgion/blob/main/skills/02-features/03-resources/13-edgion-plugins.md)
  与 [EdgionStreamPlugins —— Stream 插件](https://github.com/Pandaala/Edgion/blob/main/skills/02-features/03-resources/14-edgion-stream-plugins.md)
  ——`UnknownConfigFields` 条件的上下文说明。
- [EdgionConfigData](https://github.com/Pandaala/Edgion/blob/main/skills/01-architecture/05-resources/15-edgion-config-data.md)
  ——类型化变体、开放的 `Misc` 边界与 Controller 处理流程。
- [Admin API —— diagnostics 与 validate 接口](https://github.com/Pandaala/Edgion/blob/main/skills/01-architecture/01-controller/02-admin-api.md)
  ——完整的接口参考。
- [插件策略（允许清单 / 拒绝清单）](./plugin-policy.md)——同为插件资源上、面向运维人员可见
  的提示性功能，本检测器的条件/指标处理方式与之保持一致。
