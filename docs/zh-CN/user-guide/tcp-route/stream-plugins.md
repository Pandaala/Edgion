# Stream Plugins

> **🔌 Edgion 扩展**
>
> Stream 插件是 `EdgionStreamPlugins` CRD 提供的连接级（L4）插件；它们在 HTTP 解析之前运行，不属于标准 Gateway API。

Stream 插件在 Pingora 的 ConnectionFilter 阶段对原始 TCP/TLS/UDP 连接应用访问控制。它们在转发任何 payload 之前决定是否允许某个连接（对于 UDP，则是一个会话）。

## 附加方式

Stream 插件**不**内联在路由中。你先定义一个 `EdgionStreamPlugins` 资源，然后通过路由上的 `edgion.io/edgion-stream-plugins` 注解引用它。注解的值为 `namespace/name`（若插件与路由位于同一命名空间，则只写 `name`）。

```yaml
metadata:
  annotations:
    edgion.io/edgion-stream-plugins: "edgion-test/tcp-ip-restriction-deny"
```

同一注解适用于 `TCPRoute`、`TLSRoute`、`UDPRoute` 以及 Gateway 监听器级别。在 `UDPRoute` 上，由于 UDP 没有 TLS 握手，只会执行 Stage-1 ConnectionFilter（`spec.plugins`）。

## 支持的插件

| 类型 | 配置对象 | 说明 |
|------|----------|------|
| `IpRestriction` | 内联规则 | IP 允许/拒绝分组，直接嵌入插件中。 |
| `GlobalConnectionIpRestriction` | `pluginMetadataRef` | 引用 `GlobalConnectionIpRestriction` PluginMetaData；由 Center 管理并同步到各 Controller。 |

`spec.plugins` 下的每个条目都有可选的 `enable` 标志（默认 `true`）、一个 `type` 和一个 `config` 对象。

### `IpRestriction` 配置

| 字段 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `allow` | Array\<Object\> | 无 | 允许分组；每项格式为 `{name, cidrs}`。非空的 `allow` 启用白名单模式。 |
| `deny` | Array\<Object\> | 无 | 拒绝分组；每项格式为 `{name, cidrs}`。优先于 `allow` 求值。 |
| `defaultAction` | Enum | `allow` | 没有规则匹配时的动作。允许值：`allow` \| `deny`。 |

`allow` 和 `deny` 中至少有一个必须非空。

#### 分组子字段

| 字段 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `name` | String | **必填** | 分组名称，在插件内唯一（仅用于管理标记）。 |
| `cidrs` | Array\<String\> | **必填** | CIDR 字符串或裸 IP（裸 IP 会转为 `/32` 或 `/128`）。 |
| `description` | String | 无 | 可选的可读说明。 |

求值顺序：先 `deny`，再 `allow`，最后 `defaultAction`。对于原始 TCP，被拒绝的连接直接关闭（L4 没有协议级响应）。

### `GlobalConnectionIpRestriction` 配置

| 字段 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `pluginMetadataRef.name` | String | **必填** | `GlobalConnectionIpRestriction` PluginMetaData 的名称。 |
| `pluginMetadataRef.namespace` | String | 父 ESP 命名空间 | PluginMetaData 的命名空间；省略时继承父 `EdgionStreamPlugins` 的命名空间。 |

被引用的 PluginMetaData 在 `spec.metadata.config` 中集中存储规则，带有总开关（`enable`）、当前激活的 `activeProfile` 以及命名的 `profiles`，可即时切换规则集。这使多个 Controller 可以共享同一个策略。

## 快速开始（内联 IpRestriction）

### 第一步：定义插件

```yaml
apiVersion: edgion.io/v1
kind: EdgionStreamPlugins
metadata:
  name: tcp-ip-restriction-allow
  namespace: edgion-test
spec:
  plugins:
    - type: IpRestriction
      config:
        allow:
          - name: allowed-clients
            cidrs:
              - "10.0.0.0/8"          # 仅限内网
        defaultAction: deny           # 拒绝所有不在允许列表中的请求
```

### 第二步：在路由中引用

```yaml
apiVersion: gateway.networking.k8s.io/v1alpha2
kind: TCPRoute
metadata:
  name: redis-route
  namespace: edgion-test
  annotations:
    edgion.io/edgion-stream-plugins: "edgion-test/tcp-ip-restriction-allow"
spec:
  parentRefs:
    - name: stream-gateway
      sectionName: tcp-redis
  rules:
    - backendRefs:
        - name: redis-primary
          port: 6379
```

### 第三步：应用

```bash
kubectl apply -f stream-plugins.yaml
kubectl apply -f tcp-route.yaml
```

现在只有 `10.0.0.0/8` 内的客户端可以访问 Redis 服务。

## Center 管理的限制（GlobalConnectionIpRestriction）

将规则集中存储在一个 `GlobalConnectionIpRestriction` PluginMetaData 中（支持命名 `profiles` 和总开关），并从 Stream 插件引用它。Center 将这种形式同步到多个 Controller。

```yaml
apiVersion: edgion.io/v1
kind: PluginMetaData
metadata:
  name: gcir-policy
  namespace: edgion-test
spec:
  metadata:
    type: GlobalConnectionIpRestriction
    config:
      enable: true                    # 总开关；false 时禁用策略
      activeProfile: main             # 当前激活的 profile
      profiles:
        main:
          defaultAction: deny
          allow:
            - name: office
              cidrs:
                - "10.0.0.0/8"
---
apiVersion: edgion.io/v1
kind: EdgionStreamPlugins
metadata:
  name: gcir-stream
  namespace: edgion-test
spec:
  plugins:
    - type: GlobalConnectionIpRestriction
      config:
        pluginMetadataRef:
          name: gcir-policy
          # namespace 省略：继承父 ESP 命名空间（edgion-test）
---
apiVersion: gateway.networking.k8s.io/v1alpha2
kind: TCPRoute
metadata:
  name: gcir-route
  namespace: edgion-test
  annotations:
    edgion.io/edgion-stream-plugins: "edgion-test/gcir-stream"
spec:
  parentRefs:
    - name: stream-gateway
      sectionName: tcp-redis
  rules:
    - backendRefs:
        - name: redis-primary
          port: 6379
```

## 跨命名空间引用

使用 `namespace/name` 格式指向其他命名空间的插件：

```yaml
apiVersion: gateway.networking.k8s.io/v1alpha2
kind: TCPRoute
metadata:
  name: app-route
  namespace: app
  annotations:
    edgion.io/edgion-stream-plugins: "security/common-policy"
spec:
  parentRefs:
    - name: stream-gateway
      sectionName: tcp-redis
  rules:
    - backendRefs:
        - name: redis-primary
          port: 6379
```

## 边缘情况

### UDPRoute 上的同一注解

`UDPRoute` 接受相同的注解。只有 Stage-1（`spec.plugins`）会执行；IP 检查在每个会话的第一个数据报上求值（fail-close），因此被拒绝的客户端永远无法建立会话。

```yaml
apiVersion: gateway.networking.k8s.io/v1alpha2
kind: UDPRoute
metadata:
  name: udp-route-with-plugins
  namespace: edgion-test
  annotations:
    edgion.io/edgion-stream-plugins: "edgion-test/udp-ip-restriction-deny"
spec:
  parentRefs:
    - name: stream-gateway
      sectionName: udp-plugins
  rules:
    - backendRefs:
        - name: udp-echo-backend
          port: 9300
```

### 在允许范围内拒绝单个主机

`deny` 先于 `allow` 求值，因此你可以在允许宽泛 CIDR 的同时排除某个地址。分组名称是任意标签，仅用于组织管理。

```yaml
apiVersion: edgion.io/v1
kind: EdgionStreamPlugins
metadata:
  name: carve-out
  namespace: edgion-test
spec:
  plugins:
    - type: IpRestriction
      config:
        allow:
          - name: internal
            cidrs:
              - "10.0.0.0/8"
        deny:
          - name: quarantine
            cidrs:
              - "10.6.6.6/32"        # 尽管在 10.0.0.0/8 内，仍被拒绝
        defaultAction: deny
```

## 故障排除

### 插件未生效

1. 确认插件资源存在：
   ```bash
   kubectl get edgionstreamplugins -A
   ```
2. 检查注解值（必须为 `name` 或 `namespace/name`）：
   ```bash
   kubectl get tcproute <name> -o yaml | grep -A2 annotations
   ```
3. 查看 Gateway 日志：
   ```bash
   kubectl logs <gateway-pod> | grep -i "stream plugin"
   ```

### 连接被意外拒绝

`deny` 优先于 `allow`，`defaultAction: deny` 拒绝所有不在列表中的请求。
请验证客户端 IP 是否在某个 `allow` 分组的 `cidrs` 范围内，且未被 `deny` 分组捕获。
