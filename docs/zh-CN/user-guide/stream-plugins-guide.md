# Stream Plugins 用户指南

快速上手 Edgion 的 TCP/UDP 流式插件功能。

## 什么是 Stream Plugins？

Stream Plugins 为 TCP 和 UDP 流量提供访问控制和安全策略，例如 IP 限制、速率限制等。

**支持的协议**：
- ✅ TCP (TCPRoute)
- ✅ UDP (UDPRoute)

**当前支持的插件**：
- 🔒 **IP Restriction** - 基于客户端 IP 地址的访问控制

---

## 快速开始

### 步骤 1：创建插件配置

创建 `EdgionStreamPlugins` 资源：

```yaml
apiVersion: edgion.io/v1
kind: EdgionStreamPlugins
metadata:
  name: ip-filter
  namespace: default
spec:
  plugins:
    - type: IpRestriction
      config:
        ipSource: remoteAddr      # 使用真实连接 IP
        allow:                     # 白名单
          - "10.0.0.0/8"
          - "192.168.1.0/24"
        deny:                      # 黑名单（优先级高）
          - "10.0.0.100"
        defaultAction: deny        # 默认拒绝
        message: "Access denied"
```

### 步骤 2：在路由中引用

在 TCPRoute 的 `annotations` 中引用插件：

```yaml
apiVersion: gateway.networking.k8s.io/v1alpha2
kind: TCPRoute
metadata:
  name: my-tcp-route
  namespace: default
  annotations:
    edgion.io/stream-plugins: ip-filter  # 引用插件名称
spec:
  parentRefs:
    - name: my-gateway
      sectionName: tcp-6379
  rules:
    - backendRefs:
        - name: redis-service
          port: 6379
```

### 步骤 3：应用配置

```bash
kubectl apply -f stream-plugins.yaml
kubectl apply -f tcp-route.yaml
```

完成！现在只有白名单中的 IP 可以访问你的 TCP 服务。

---

## IP Restriction 配置详解

### 基本配置项

| 字段 | 类型 | 说明 | 必填 |
|------|------|------|------|
| `ipSource` | string | IP 来源：`remoteAddr`（连接 IP） | 是 |
| `allow` | []string | IP 白名单（CIDR 格式） | 否 |
| `deny` | []string | IP 黑名单（优先级高于白名单） | 否 |
| `defaultAction` | string | 默认动作：`allow` 或 `deny` | 是 |
| `message` | string | 拒绝访问时的提示消息 | 否 |

### 判断逻辑

```
1. 检查 deny 列表 → 匹配则拒绝
2. 检查 allow 列表 → 匹配则允许
3. 应用 defaultAction
```

---

## 使用场景

### 场景 1：数据库访问控制

只允许内网 IP 访问 Redis：

```yaml
apiVersion: edgion.io/v1
kind: EdgionStreamPlugins
metadata:
  name: redis-security
spec:
  plugins:
    - type: IpRestriction
      config:
        ipSource: remoteAddr
        allow:
          - "10.0.0.0/8"      # 内网
        defaultAction: deny
```

### 场景 2：封禁特定 IP

允许所有人访问，但封禁恶意 IP：

```yaml
apiVersion: edgion.io/v1
kind: EdgionStreamPlugins
metadata:
  name: block-bad-ips
spec:
  plugins:
    - type: IpRestriction
      config:
        ipSource: remoteAddr
        deny:
          - "1.2.3.4"
          - "5.6.7.0/24"
        defaultAction: allow
```

### 场景 3：多路由共享策略

一个插件配置被多个路由复用：

```yaml
# 定义一次
apiVersion: edgion.io/v1
kind: EdgionStreamPlugins
metadata:
  name: common-policy
spec:
  plugins:
    - type: IpRestriction
      config:
        allow: ["10.0.0.0/8"]
        defaultAction: deny

---
# TCP 路由使用
apiVersion: gateway.networking.k8s.io/v1alpha2
kind: TCPRoute
metadata:
  annotations:
    edgion.io/stream-plugins: common-policy
# ...

---
# UDP 路由也使用
apiVersion: gateway.networking.k8s.io/v1alpha2
kind: UDPRoute
metadata:
  annotations:
    edgion.io/stream-plugins: common-policy
# ...
```

---

## 跨命名空间引用

插件和路由可以在不同的命名空间，使用 `namespace/name` 格式：

```yaml
# 插件在 security 命名空间
apiVersion: edgion.io/v1
kind: EdgionStreamPlugins
metadata:
  name: global-policy
  namespace: security
spec:
  plugins:
    - type: IpRestriction
      config:
        allow: ["10.0.0.0/8"]
        defaultAction: deny

---
# 路由在 app 命名空间，跨命名空间引用
apiVersion: gateway.networking.k8s.io/v1alpha2
kind: TCPRoute
metadata:
  name: app-route
  namespace: app
  annotations:
    edgion.io/stream-plugins: security/global-policy  # 跨命名空间
spec:
  # ...
```

---

## 故障排查

### 插件未生效

**检查清单**：

1. 确认插件资源存在：
   ```bash
   kubectl get edgionstreamplugins -A
   ```

2. 检查 annotation 是否正确：
   ```bash
   kubectl get tcproute <name> -o yaml | grep annotations -A 2
   ```

3. 查看 Gateway 日志：
   ```bash
   kubectl logs <gateway-pod> | grep -i "stream plugin"
   ```

4. 验证命名空间匹配：
   - 同命名空间：直接使用插件名
   - 跨命名空间：使用 `namespace/name` 格式

### 连接被拒绝

检查 IP 是否在白名单中：

```bash
# 查看你的 IP
curl ifconfig.me

# 查看插件配置
kubectl get edgionstreamplugins <name> -o yaml
```

---

## 性能考虑

- ✅ IP 检查在连接建立时执行，不影响数据传输性能
- ✅ 使用 CIDR 匹配算法，查询速度快
- ✅ 插件配置热更新，无需重启 Gateway

---

## 下一步

- 📖 [完整 Annotations 参考](../developer-doc/annotations-guide.md)
- 🔧 [添加自定义插件](../developer-doc/add-new-resource-guide.md)
- 🏗️ [架构设计](../developer-doc/architecture-overview.md)

---

**版本**: Edgion v0.1.0  
**最后更新**: 2025-12-25
