# 请求访问策略

`RequestAccessPolicy` 是纯七层请求插件，可以挂载到 Gateway，也可以通过 `ExtensionRef` 只挂载到某条 HTTPRoute。

## 策略模型

配置包含多个命名 profile，并通过 `defaultProfile` 指定默认项。可选的 `activeProfileRef` 引用 Selector 类型的 `EdgionConfigData`；Selector 不可用属于硬失败，请求会被拒绝。

每个 profile 包含 `requiredRuleGroups`，所有 group 都必须通过（AND）。每个 group 内的 `anyOfRules` 只需有一条完整规则通过（OR），而且同一 group 可以混合三种规则：

- `directPeerIp`：先匹配当前 TCP 连接的直接上一跳，再检查所有 `allOf` 条件。
- `remoteIp`：先匹配 RealIp 解析出的客户端地址，再检查所有 `allOf` 条件。
- `url`：先匹配 path，再匹配精确 host 和所有 `allOf` 条件。

Controller 会把一条完整 IP 规则中的内联 IP 与所有有效 `ipRefs` 合并，并统一编译成一棵 radix tree。`configRefs` 引用强类型 `RequestAccessUrlAllowList`。IP/URL 数据引用不存在、未启用、类型错误或跨命名空间未授权时，只忽略该引用并记录 `PartiallyInvalid`，其他有效引用继续生效。ConfigData 或 Secret 更新后会自动 requeue 对应的 `EdgionPlugins` 并重新生成运行时快照。

`allOf` 使用与插件 `run` / `skip` 相同的 `Condition` 枚举（含 `timeRange` 与 `probability`）。`keyMatch` 使用与插件条件相同的 `MatchCore`（`values` / `regex` / `caseSensitive`，没有单数 `value`）。详见 [插件条件](./conditions.md)。Secret/HMAC 在 Controller 解析，管理配置输出中的敏感值会被脱敏。`hmacAuth.validateRequestBody` 只在此处（以及日后的 RequestRestriction）合法，条目级条件会拒绝它。

### HMAC 条件

`hmacAuth` 条件复用独立 `HmacAuth` 插件的凭据加载和签名校验，但只负责匹配。它支持 `secretGroups`、`algorithms`、`clockSkew`、`enforceHeaders`、`validateRequestBody`、`minSecretLength`、`secretField` 和 `usernameField`。`anonymous`、`realm`、`authFailureDelayMs`、`hideCredentials`、`upstreamHeaderFields` 等独立插件行为字段会被拒绝。

```yaml
allOf:
  - type: hmacAuth
    secretGroups:
      - name: vendors
        secretRefs:
          - name: vendor-hmac-credentials
    algorithms: [hmac-sha256]
    enforceHeaders: [date, "@request-target"]
    validateRequestBody: true
```

签名缺失或无效、凭据未知、请求体摘要不匹配时，该条件判定为不匹配，同一 `anyOfRules` group 中的其他规则仍可继续授权。请求体捕获或存储失败可能导致没有可继续转发的请求体，因此会通过网关统一的 body 错误响应终止整个策略，不会回退到同组其他规则。策略条件不会注入凭据 metadata header。

## 示例

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: dev-access
  namespace: edgion-system
spec:
  requestPlugins:
    - type: RequestAccessPolicy
      config:
        defaultProfile: lockdown
        status: 403
        profiles:
          lockdown:
            requiredRuleGroups:
              - name: trusted-previous-hop
                anyOfRules:
                  - name: internal-peer
                    type: directPeerIp
                    config:
                      ipRefs: [{ name: internal-networks }]
                  - name: authenticated-cdn
                    type: directPeerIp
                    config:
                      ipRefs: [{ name: cloudflare-networks }]
                      allOf:
                        - type: secretMatch
                          key: { type: header, name: X-Origin-Token }
                          secretRef: { name: cloudflare-origin-token }
                          secretKey: token
              - name: allowed-client
                anyOfRules:
                  - name: internal-client
                    type: remoteIp
                    config:
                      ipRefs: [{ name: internal-networks }]
                  - name: vendor-callback
                    type: url
                    config:
                      hosts: [callbacks.example.com]
                      paths:
                        - type: PathPrefix
                          value: /callbacks/:vendor
                      allOf:
                        - type: remoteIpMatch
                          ipRefs: [{ name: vendor-networks }]
```

`Exact` 和 `PathPrefix` 支持路由风格的 `:参数` 路径段，并统一编译到 profile 级 radix tree。host 会去掉端口和末尾 DNS 点、转为小写后精确匹配，不支持通配 host。

插件固定 fail-close。每个 required group 都必须找到一条成功的完整规则；配置错误或 profile 选择失败都会拒绝请求。
