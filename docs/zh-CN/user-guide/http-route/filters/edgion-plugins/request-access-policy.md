# 请求访问策略

`RequestAccessPolicy` 是纯七层请求插件，可通过 Gateway 的插件引用作用于整个网关，也可通过 HTTPRoute 的 `ExtensionRef` 只作用于某条路由。

## 策略模型

配置包含多个命名 profile，并通过 `defaultProfile` 指定默认项。可选的 `activeProfileRef` 引用 Selector 类型的 `EdgionConfigData`，用于热切换 profile。Selector 不存在、类型错误、跨命名空间引用被拒绝或选中了未知 profile 时，请求会被拒绝。

每个 profile 分别判断两个 IP 身份：

- `directPeerIp`：当前 TCP 连接的直接上一跳，不会被请求头改写，安全规则应优先使用它。
- `remoteIp`：RealIp 根据可信代理配置解析出的客户端地址；可信代理配置错误时可能被伪造。

`sourceMatch.operator` 默认为 `any`。设为 `all` 时必须同时配置两个 IP 分支，并且两个分支都要匹配。

每个 IP 分支包含两类互为 OR 的规则：

- `ipRules`：先匹配 IP，再判断可选的辅助条件。
- `urlRules`：先匹配精确 host 和 path，再判断可选的辅助条件。Exact 和 PathPrefix 使用 radix tree，并支持 `/callbacks/:vendor` 形式的参数段。

不写 `anyOf` 表示主条件匹配即可。写了以后，`anyOf` 各项之间是 OR，每项内部的 `allOf` 是 AND；空数组会在配置校验阶段被拒绝。

## 辅助条件

`allOf` 支持：

- `keyExist`：本地 `KeyGet` 值存在。
- `keyMatch`：本地 `KeyGet` 值匹配 `value`、`values` 或 `regex`。
- `secretMatch`：本地 `KeyGet` 值与 Secret 中任一值精确相等。
- `ipMatch`：当前分支的 IP 命中内联或 ConfigData IP 列表。
- `hmacAuth`：通过标准 HMAC 校验；嵌入策略时不会修改请求头。

策略条件禁止使用 Webhook 类型的 KeyGet，避免访问控制依赖运行时外部网络调用。解析后的 Secret 和 HMAC 凭据在管理配置输出中会被脱敏。

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
        message: Access denied
        profiles:
          lockdown:
            defaultAction: deny
            sourceMatch:
              operator: any
              directPeerIp:
                ipRules:
                  - name: internal
                    ips:
                      refs:
                        - name: internal-networks
                  - name: cdn
                    ips:
                      refs:
                        - name: cdn-networks
                    anyOf:
                      - allOf:
                          - type: secretMatch
                            key:
                              type: header
                              name: X-Origin-Token
                            secretRef:
                              name: cdn-origin-token
                urlRules:
                  - name: vendor-callback
                    urls:
                      - hosts: ["callbacks.example.com"]
                        paths:
                          - type: PathPrefix
                            value: /callbacks/:vendor
                    anyOf:
                      - allOf:
                          - type: ipMatch
                            refs:
                              - name: vendor-networks
```

host 会先去掉端口和结尾的 DNS 点并转成小写，然后做精确匹配；不支持通配 host。PathPrefix 只在路径段边界匹配，因此 `/api` 不会匹配 `/apiv2`。

## 失败策略

插件固定 fail-close。初始化失败、profile/Selector 不可用、IP 列表引用未解析、Secret 值缺失或 HMAC 校验失败，都不能放行请求。正常完成规则判断但没有命中时，才使用 profile 的 `defaultAction`。
