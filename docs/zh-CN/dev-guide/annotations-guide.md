# Annotations 使用指南

本文档介绍 Edgion 中通过 Annotations 引用 Stream Plugins 的设计和使用方法。

## 背景

Kubernetes Gateway API 的 TCPRoute 和 UDPRoute 规范中不包含 `filters` 或 `extensionRef` 字段。为了在不违反 Gateway API 规范的前提下扩展功能，Edgion 采用 Kubernetes Annotations 机制来实现 Stream Plugins 的引用。

## 设计原则

### 1. 符合 Gateway API 规范

- ✅ 不修改 Gateway API 标准字段
- ✅ 使用 Kubernetes 原生 Annotations 机制
- ✅ 保持与标准 Gateway API 资源的兼容性

### 2. 简洁易用

- 通过单个 annotation key 引用插件
- 支持同命名空间和跨命名空间引用
- 配置直观，易于理解

### 3. 灵活性

- 插件和路由解耦，便于复用
- 支持动态更新（热重载）
- 便于集中管理安全策略

---

## Gateway Annotations

以下 annotations 用于配置 Gateway 级别的行为：

| Annotation | 类型 | 默认值 | 说明 |
|------------|------|--------|------|
| `edgion.com/enable-http2` | string | `"true"` | 控制 HTTP/2 支持（h2c 和 ALPN） |
| `edgion.io/backend-protocol` | string | - | TLS 监听器的后端协议（设为 `"tcp"` 启用 TLS 终止到 TCP） |
| `edgion.io/http-to-https-redirect` | string | `"false"` | 设为 `"true"` 启用 HTTP 到 HTTPS 重定向 |
| `edgion.io/https-redirect-port` | string | `"443"` | HTTPS 重定向目标端口 |

详细使用说明请参考 [HTTP to HTTPS 重定向指南](../user-guide/http-to-https-redirect-guide.md)。

---

## Route Annotations

### Stream Plugins Annotation

**Key**: `edgion.io/stream-plugins`

**值格式**：
- 同命名空间引用：`<plugin-name>`
- 跨命名空间引用：`<namespace>/<plugin-name>`

**适用资源**：
- `TCPRoute` (Gateway API v1alpha2)
- `UDPRoute` (Gateway API v1alpha2)

---

## 使用示例

### 1. 同命名空间引用

最常见的场景，插件和路由在同一命名空间：

```yaml
# EdgionStreamPlugins 定义
apiVersion: edgion.io/v1
kind: EdgionStreamPlugins
metadata:
  name: redis-ip-filter
  namespace: default
spec:
  plugins:
    - type: IpRestriction
      config:
        allow:
          - "10.0.0.0/8"
        defaultAction: deny

---
# TCPRoute 引用插件
apiVersion: gateway.networking.k8s.io/v1alpha2
kind: TCPRoute
metadata:
  name: redis-route
  namespace: default
  annotations:
    edgion.io/stream-plugins: redis-ip-filter  # 直接使用插件名
spec:
  parentRefs:
    - name: example-gateway
      sectionName: tcp-redis
  rules:
    - backendRefs:
        - name: redis-service
          port: 6379
```

### 2. 跨命名空间引用

安全团队在专门命名空间管理插件，业务团队跨命名空间引用：

```yaml
# 插件在 security-policies 命名空间
apiVersion: edgion.io/v1
kind: EdgionStreamPlugins
metadata:
  name: strict-ip-filter
  namespace: security-policies
spec:
  plugins:
    - type: IpRestriction
      config:
        allow:
          - "10.0.0.0/8"
        defaultAction: deny
        message: "Only internal network access allowed"

---
# 应用在 app-production 命名空间引用
apiVersion: gateway.networking.k8s.io/v1alpha2
kind: TCPRoute
metadata:
  name: app-tcp-route
  namespace: app-production
  annotations:
    edgion.io/stream-plugins: security-policies/strict-ip-filter  # 跨命名空间
spec:
  parentRefs:
    - name: prod-gateway
  rules:
    - backendRefs:
        - name: app-backend
          port: 8080
```

### 3. 多个路由共享同一插件

```yaml
# 定义一次插件
apiVersion: edgion.io/v1
kind: EdgionStreamPlugins
metadata:
  name: common-security
  namespace: default
spec:
  plugins:
    - type: IpRestriction
      config:
        allow: ["10.0.0.0/8", "172.16.0.0/12"]
        defaultAction: deny

---
# 多个 TCPRoute 共享
apiVersion: gateway.networking.k8s.io/v1alpha2
kind: TCPRoute
metadata:
  name: service-a-route
  annotations:
    edgion.io/stream-plugins: common-security
spec:
  # ...

---
apiVersion: gateway.networking.k8s.io/v1alpha2
kind: TCPRoute
metadata:
  name: service-b-route
  annotations:
    edgion.io/stream-plugins: common-security
spec:
  # ...

---
# UDPRoute 也可以使用
apiVersion: gateway.networking.k8s.io/v1alpha2
kind: UDPRoute
metadata:
  name: udp-service-route
  annotations:
    edgion.io/stream-plugins: common-security
spec:
  # ...
```

### 4. 不使用插件

如果不需要插件，只需不添加 annotation 即可：

```yaml
apiVersion: gateway.networking.k8s.io/v1alpha2
kind: TCPRoute
metadata:
  name: public-tcp-route
  namespace: default
  # 没有 edgion.io/stream-plugins annotation
spec:
  parentRefs:
    - name: public-gateway
  rules:
    - backendRefs:
        - name: public-service
          port: 80
```

---

## 实现原理

### 处理流程

```
1. ConfigManager 加载 TCPRoute/UDPRoute
   ↓
2. 检查 metadata.annotations["edgion.io/stream-plugins"]
   ↓
3. 解析引用格式（namespace/name 或 name）
   ↓
4. 从 StreamPluginStore 获取对应的 EdgionStreamPlugins
   ↓
5. 将 stream_plugin_runtime 注入到 Route 的每个 Rule
   ↓
6. 连接建立时，执行 stream_plugin_runtime.run()
```

### 代码位置

**解析逻辑**：
- `src/core/routes/tcp_routes/conf_handler_impl.rs`
- `src/core/routes/udp_routes/conf_handler_impl.rs`

**关键方法**：

```rust
impl TCPRoute {
    pub fn init_stream_plugins(&mut self, plugin_store: &StreamPluginStore) {
        if let Some(annotations) = &self.metadata.annotations {
            if let Some(plugin_ref) = annotations.get("edgion.io/stream-plugins") {
                // 解析 namespace/name 或 name
                let (plugin_namespace, plugin_name) = parse_plugin_ref(plugin_ref, route_ns);
                
                // 从 store 获取插件
                if let Some(plugins) = plugin_store.get_by_ns_name(plugin_namespace, plugin_name) {
                    for rule in self.spec.rules.iter_mut() {
                        rule.stream_plugin_runtime = plugins.spec.stream_plugin_runtime.clone();
                    }
                }
            }
        }
    }
}
```

**执行逻辑**（TCP 示例）：

```rust
// src/core/routes/tcp_routes/edgion_tcp.rs
async fn handle_connection(&self, downstream: Stream, ctx: &mut TcpContext) {
    // 1. 匹配 TCPRoute
    let tcp_route = match_tcp_route(ctx);
    
    // 2. 获取第一个 rule（通常只有一个）
    if let Some(rule) = tcp_route.spec.rules.first() {
        // 3. 检查是否有 stream plugins
        if !rule.stream_plugin_runtime.is_empty() {
            let client_ip = extract_client_ip(&downstream);
            let stream_ctx = StreamContext {
                client_ip,
                listener_port: self.listener_port,
            };
            
            // 4. 执行插件链
            match rule.stream_plugin_runtime.run(&stream_ctx).await {
                StreamPluginResult::Allow => {
                    // 继续处理连接
                }
                StreamPluginResult::Deny(reason) => {
                    tracing::info!("Connection denied by plugin: {}", reason);
                    return; // 拒绝连接
                }
            }
        }
    }
    
    // 5. 选择 backend 并建立连接
    let backend = rule.backend_finder.select(ctx);
    proxy_to_backend(downstream, backend).await;
}
```

---

## 配置管理

### 热重载

插件配置支持热重载，无需重启 Gateway：

1. 修改 `EdgionStreamPlugins` 资源
2. ConfigServer 检测到变更
3. 更新 StreamPluginStore
4. 新连接立即使用新配置
5. 已建立的连接不受影响（TCP/UDP 长连接特性）

### 插件更新示例

```bash
# 修改插件配置
kubectl edit edgionstreamplugins redis-ip-filter

# 或应用新配置
kubectl apply -f updated-plugin.yaml

# 新连接立即生效，无需重启 Gateway
```

---

## 最佳实践

### 1. 命名规范

**插件命名**：
- 描述性名称：`<service>-<policy-type>`
- 示例：`redis-ip-filter`, `mysql-rate-limit`, `strict-security`

**Annotation 值**：
- 同命名空间：直接使用插件名
- 跨命名空间：使用 `namespace/name` 格式

### 2. 插件组织

**按用途分类**：

```
security-policies namespace:
  - strict-ip-filter         # 严格 IP 限制
  - public-rate-limit        # 公开服务限流
  - internal-only            # 仅内网访问

default namespace:
  - dev-ip-filter            # 开发环境
  - staging-ip-filter        # 测试环境
```

### 3. 权限控制

使用 Kubernetes RBAC 控制插件资源的访问：

```yaml
# 示例：允许 app-team 使用但不能修改 security-policies 中的插件
apiVersion: rbac.authorization.k8s.io/v1
kind: Role
metadata:
  name: plugin-reader
  namespace: security-policies
rules:
  - apiGroups: ["edgion.io"]
    resources: ["edgionstreamplugins"]
    verbs: ["get", "list", "watch"]  # 只读权限
```

### 4. 文档和注释

在插件资源中添加详细描述：

```yaml
apiVersion: edgion.io/v1
kind: EdgionStreamPlugins
metadata:
  name: production-security
  namespace: security-policies
  annotations:
    description: "生产环境标准安全策略"
    owner: "security-team@company.com"
    last-review: "2025-12-25"
spec:
  plugins:
    - type: IpRestriction
      config:
        # 配置详情...
```

---

## 常见问题

### 插件未生效

**问题**：配置了 annotation 但插件没有执行

**排查步骤**：

1. **检查 annotation key 是否正确**：
   ```bash
   kubectl get tcproute <name> -o yaml | grep annotations -A 2
   ```
   确认是 `edgion.io/stream-plugins`（注意拼写和域名）

2. **检查插件资源是否存在**：
   ```bash
   # 同命名空间
   kubectl get edgionstreamplugins <plugin-name> -n <route-namespace>
   
   # 跨命名空间
   kubectl get edgionstreamplugins <plugin-name> -n <plugin-namespace>
   ```

3. **检查 Gateway 日志**：
   ```bash
   kubectl logs <gateway-pod> | grep -i "stream plugin"
   ```
   
   可能的日志信息：
   - `"EdgionStreamPlugins not found: <name>"` - 插件不存在
   - `"Loading stream plugins for TCPRoute <name>"` - 正常加载
   - `"Connection denied by plugin: ..."` - 插件执行并拒绝连接

### 命名空间问题

**问题**：跨命名空间引用失败

**解决**：
- 确认格式正确：`namespace/name`
- 检查插件资源确实存在于目标命名空间

**错误示例**：
```yaml
annotations:
  edgion.io/stream-plugins: my-plugin  # ❌ 如果插件在其他命名空间
```

**正确示例**：
```yaml
annotations:
  edgion.io/stream-plugins: security-policies/my-plugin  # ✅ 明确指定命名空间
```

### 插件更新不生效

**问题**：修改了 EdgionStreamPlugins，但路由行为没变化

**原因**：
- 已建立的 TCP/UDP 连接使用旧配置（长连接特性）
- ConfigServer 可能还未同步更新

**解决**：
1. 等待新连接：新建立的连接会使用新配置
2. 检查 ConfigServer 同步状态
3. 确认插件资源的 `resourceVersion` 已更新

---

## 与 HTTPRoute 的区别

### HTTPRoute 使用 `filters`

HTTPRoute 规范包含 `filters` 字段，可以直接引用插件：

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
spec:
  rules:
    - filters:
        - type: ExtensionRef
          extensionRef:
            group: edgion.io
            kind: EdgionPlugins
            name: my-http-plugin
```

### TCPRoute/UDPRoute 使用 Annotations

由于 TCPRoute/UDPRoute 规范不包含 `filters`，使用 annotations：

```yaml
apiVersion: gateway.networking.k8s.io/v1alpha2
kind: TCPRoute
metadata:
  annotations:
    edgion.io/stream-plugins: my-stream-plugin
spec:
  rules:
    - backendRefs: [...]
```

**对比总结**：

| 特性 | HTTPRoute | TCPRoute/UDPRoute |
|------|-----------|-------------------|
| 引用方式 | `spec.rules.filters` | `metadata.annotations` |
| 规范支持 | ✅ Gateway API 标准 | ❌ 需要扩展机制 |
| 实现方式 | ExtensionRef | Annotations |
| 灵活性 | 每个 rule 可配置不同 filter | 整个 Route 共享插件 |
| 粒度 | 细粒度（per-rule） | 粗粒度（per-route） |

---

## 未来计划

### 1. 支持多插件引用

当前一个 Route 只能引用一个 EdgionStreamPlugins 资源。未来可能支持：

```yaml
annotations:
  edgion.io/stream-plugins: "ip-filter,rate-limit,audit-log"
```

### 2. Per-Rule 插件配置

探索更细粒度的插件配置：

```yaml
annotations:
  edgion.io/stream-plugins.rule-0: "strict-filter"
  edgion.io/stream-plugins.rule-1: "loose-filter"
```

### 3. 插件优先级

支持多个插件的执行顺序控制：

```yaml
spec:
  plugins:
    - type: IpRestriction
      priority: 100  # 先执行
    - type: RateLimit
      priority: 50   # 后执行
```

---

## 参考资料

- [Kubernetes Gateway API - TCPRoute](https://gateway-api.sigs.k8s.io/api-types/tcproute/)
- [Kubernetes Gateway API - UDPRoute](https://gateway-api.sigs.k8s.io/api-types/udproute/)
- [Kubernetes Annotations](https://kubernetes.io/docs/concepts/overview/working-with-objects/annotations/)
- [Edgion Architecture Overview](./architecture-overview.md)
- [Stream Plugins User Guide](../user-guide/stream-plugins-guide.md)

---

**版本**: Edgion v0.1.0  
**最后更新**: 2025-12-25

