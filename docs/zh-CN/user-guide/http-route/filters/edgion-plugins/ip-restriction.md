# IP Restriction 插件

> **🔌 Edgion 扩展**
> 
> IpRestriction 是 `EdgionPlugins` CRD 提供的访问控制插件，不属于标准 Gateway API。

## 什么是 IP Restriction？

IP Restriction（IP 限制）插件用于控制哪些 IP 地址或网段可以访问你的 API，提供白名单和黑名单功能。

**使用场景**：
- 只允许内网 IP 访问管理接口
- 禁止特定恶意 IP 访问
- 允许特定合作伙伴 IP 访问 API
- 限制支付接口只能从应用服务器访问

## 快速开始

### 白名单模式（只允许特定 IP）

```yaml
filters:
  - type: IpRestriction
    config:
      allow:
        - "192.168.1.0/24"  # 允许整个子网
        - "10.0.0.100"       # 允许单个 IP
```

**效果**：只有 `192.168.1.x` 和 `10.0.0.100` 可以访问，其他所有 IP 被拒绝。

### 黑名单模式（拒绝特定 IP）

```yaml
filters:
  - type: IpRestriction
    config:
      deny:
        - "203.0.113.50"     # 拒绝单个恶意 IP
        - "198.51.100.0/24"  # 拒绝整个恶意子网
```

**效果**：只有列表中的 IP 被拒绝，其他所有 IP 都可以访问。

---

## 配置参数

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `allow` | Array | 无 | 白名单。IP 地址或 CIDR 列表。示例：`["192.168.1.0/24", "10.0.0.1"]` |
| `deny` | Array | 无 | 黑名单。IP 地址或 CIDR 列表。示例：`["203.0.113.50"]` |
| `ipSource` | String | `"clientIp"` | IP 来源。`clientIp`：从代理头提取真实 IP；`remoteAddr`：使用 TCP 连接地址 |
| `status` | Integer | `403` | 拒绝时返回的 HTTP 状态码 |
| `message` | String | 无 | 拒绝时返回的自定义消息 |
| `defaultAction` | String | `"allow"` | 默认动作。当 IP 不匹配任何规则时的行为。`allow` 或 `deny` |

---

## 优先级规则

插件使用与 Nginx 一致的三层优先级规则：

```
1. Deny 优先（最高优先级）
   └─> 如果 IP 在 deny 列表，直接拒绝

2. Allow 次之
   └─> 如果 IP 在 allow 列表，允许访问
   └─> 如果配置了 allow 但 IP 不在列表，拒绝

3. Default Action（兜底）
   └─> 如果都不匹配，使用 defaultAction
```

### 示例说明

```yaml
allow: ["10.0.0.0/8"]
deny: ["10.0.0.100"]
defaultAction: "deny"
```

| 客户端 IP | 匹配规则 | 结果 |
|-----------|----------|------|
| `10.0.0.100` | 在 deny 列表 | ❌ 拒绝（deny 优先） |
| `10.0.0.50` | 在 allow 列表 | ✅ 允许 |
| `10.1.2.3` | 在 allow 列表 | ✅ 允许 |
| `192.168.1.1` | 不在任何列表 | ❌ 拒绝（defaultAction: deny） |

---

## 常见配置场景

### 1. 内网专用 API

只允许公司内网访问：

```yaml
filters:
  - type: IpRestriction
    config:
      allow:
        - "10.0.0.0/8"        # 私网 A 类
        - "172.16.0.0/12"     # 私网 B 类
        - "192.168.0.0/16"    # 私网 C 类
      message: "Access denied: Internal network only"
      status: 403
```

### 2. 办公室白名单

只允许办公室固定 IP：

```yaml
filters:
  - type: IpRestriction
    config:
      allow:
        - "203.0.113.10"      # 办公室 IP 1
        - "203.0.113.20"      # 办公室 IP 2
        - "198.51.100.0/24"   # VPN 网段
      message: "Access restricted to office network"
```

### 3. 黑名单：封禁恶意 IP

```yaml
filters:
  - type: IpRestriction
    config:
      deny:
        - "203.0.113.50"      # 恶意攻击者
        - "198.51.100.100"    # 恶意爬虫
        - "192.0.2.0/24"      # 恶意网段
      message: "Your IP has been blocked"
      status: 403
      defaultAction: "allow"  # 其他 IP 允许访问
```

### 4. 组合模式：允许子网但排除特定 IP

允许整个子网，但排除几个 IP（如：测试机器）：

```yaml
filters:
  - type: IpRestriction
    config:
      allow:
        - "10.0.0.0/16"       # 允许整个子网
      deny:
        - "10.0.0.100"        # 排除测试机器 1
        - "10.0.0.200"        # 排除测试机器 2
```

**结果**：
- `10.0.0.50` → ✅ 允许（在 allow，不在 deny）
- `10.0.0.100` → ❌ 拒绝（在 deny，deny 优先）
- `192.168.1.1` → ❌ 拒绝（不在 allow）

### 5. 合作伙伴 API

允许特定合作伙伴访问：

```yaml
filters:
  - type: IpRestriction
    config:
      allow:
        - "203.0.113.0/24"    # 合作伙伴 A
        - "198.51.100.50"     # 合作伙伴 B
        - "192.0.2.100"       # 合作伙伴 C
      message: "Access restricted to authorized partners"
      status: 403
```

### 6. 多层次访问控制

不同路径不同的 IP 限制：

```yaml
# 管理接口：只允许内网
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: admin-api
spec:
  rules:
    - matches:
        - path:
            type: PathPrefix
            value: /admin
      filters:
        - type: ExtensionRef
          extensionRef:
            group: edgion.io
            kind: EdgionPlugins
            name: admin-ip-policy
---
# 公开接口：黑名单模式
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: public-api
spec:
  rules:
    - matches:
        - path:
            type: PathPrefix
            value: /api
      filters:
        - type: ExtensionRef
          extensionRef:
            group: edgion.io
            kind: EdgionPlugins
            name: public-ip-policy
---
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: admin-ip-policy
spec:
  plugins:
    - enable: true
      plugin:
        type: IpRestriction
        config:
          allow:
            - "10.0.0.0/8"
---
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: public-ip-policy
spec:
  plugins:
    - enable: true
      plugin:
        type: IpRestriction
        config:
          deny:
            - "203.0.113.50"
          defaultAction: "allow"
```

---

## IP 来源选择

### `ipSource: clientIp`（默认，推荐）

从代理头（`X-Forwarded-For` 或 `X-Real-IP`）提取真实客户端 IP。

**使用场景**：
- 应用部署在 CDN/负载均衡器后
- 需要获取真实客户端 IP

**示例**：
```
客户端 IP: 203.0.113.50
 ↓
CDN/负载均衡器: 198.51.100.10
 ↓
Edgion 提取: X-Forwarded-For: 203.0.113.50
 ↓
匹配规则: 203.0.113.50
```

```yaml
filters:
  - type: IpRestriction
    config:
      ipSource: "clientIp"  # 默认值
      allow:
        - "203.0.113.0/24"
```

### `ipSource: remoteAddr`

使用 TCP 连接的对端地址（直连 IP）。

**使用场景**：
- 直接部署，没有代理/负载均衡器
- 需要限制代理服务器 IP

**示例**：
```
负载均衡器: 10.0.0.50
 ↓
Edgion 获取: TCP peer address: 10.0.0.50
 ↓
匹配规则: 10.0.0.50
```

```yaml
filters:
  - type: IpRestriction
    config:
      ipSource: "remoteAddr"
      allow:
        - "10.0.0.0/8"  # 只允许内网负载均衡器
```

---

## CIDR 表示法

### 单个 IP

```yaml
allow:
  - "192.168.1.100"      # 单个 IP
  - "203.0.113.50"
```

### CIDR 子网

```yaml
allow:
  - "192.168.1.0/24"     # 192.168.1.0 - 192.168.1.255 (256个IP)
  - "10.0.0.0/8"         # 10.0.0.0 - 10.255.255.255 (16M个IP)
  - "172.16.0.0/12"      # 172.16.0.0 - 172.31.255.255
  - "203.0.113.0/28"     # 203.0.113.0 - 203.0.113.15 (16个IP)
```

### CIDR 速查表

| CIDR | 子网掩码 | IP 数量 | IP 范围示例 |
|------|----------|---------|-------------|
| `/32` | 255.255.255.255 | 1 | 单个 IP |
| `/24` | 255.255.255.0 | 256 | x.x.x.0 - x.x.x.255 |
| `/16` | 255.255.0.0 | 65,536 | x.x.0.0 - x.x.255.255 |
| `/8` | 255.0.0.0 | 16,777,216 | x.0.0.0 - x.255.255.255 |

### IPv6 支持

```yaml
allow:
  - "2001:db8::1"                    # 单个 IPv6
  - "2001:db8::/32"                  # IPv6 子网
  - "fe80::/10"                      # 本地链路地址
```

---

## 自定义拒绝响应

### 默认响应

```
HTTP/1.1 403 Forbidden
Content-Type: application/json

{
  "error": "IP address not allowed"
}
```

### 自定义状态码和消息

```yaml
filters:
  - type: IpRestriction
    config:
      allow:
        - "10.0.0.0/8"
      status: 404  # 假装资源不存在（隐藏 API）
      message: "Not Found"
```

或者返回更友好的消息：

```yaml
filters:
  - type: IpRestriction
    config:
      allow:
        - "192.168.1.0/24"
      status: 403
      message: "Access denied. Please contact administrator at admin@example.com"
```

---

## 安全最佳实践

### ✅ 推荐做法

1. **最小权限原则**
   ```yaml
   # ✅ 好：只允许需要的 IP
   allow:
     - "192.168.1.100"
     - "192.168.1.101"
   
   # ❌ 差：允许整个互联网
   # defaultAction: "allow"
   ```

2. **使用最小的 CIDR 范围**
   ```yaml
   # ✅ 好：精确范围
   allow:
     - "10.0.1.0/24"
   
   # ❌ 差：过大范围
   allow:
     - "10.0.0.0/8"
   ```

3. **定期审查规则**
    - 移除不再需要的 IP
    - 更新变更的办公室 IP

4. **记录拒绝原因**
   ```yaml
   message: "Internal API - Access restricted to office network (203.0.113.0/24)"
   ```

5. **结合其他认证**
   ```yaml
   # IP 限制 + Basic Auth = 双重保护
   filters:
     - type: IpRestriction
       config:
         allow: ["10.0.0.0/8"]
     - type: BasicAuth
       config:
         secretRefs:
           - name: admin-users
   ```

### ❌ 避免做法

1. **不要在生产环境使用过大范围**
   ```yaml
   # ❌ 危险：允许所有私网
   allow:
     - "0.0.0.0/0"  # 允许所有 IP
   ```

2. **不要仅依赖 IP 限制做认证**
    - IP 可以伪造（特别是在内网）
    - 应结合用户认证使用

3. **不要忘记 IPv6**
   ```yaml
   # 如果支持 IPv6，同时配置
   allow:
     - "192.168.1.0/24"   # IPv4
     - "2001:db8::/32"    # IPv6
   ```

---

## 故障排除

### 问题 1：合法 IP 被拒绝

**原因**：
- CIDR 范围配置错误
- IP 来源配置错误（`ipSource`）
- 有 CDN/负载均衡器但使用了 `remoteAddr`

**解决方案**：
```bash
# 查看请求日志，确认实际 IP
# 检查 X-Forwarded-For 头

# 调整配置
ipSource: "clientIp"  # 如果有代理
# 或
ipSource: "remoteAddr"  # 如果直连
```

### 问题 2：无法获取真实 IP

**原因**：CDN/负载均衡器未正确设置代理头

**解决方案**：
```yaml
# 临时：使用 remoteAddr 并允许负载均衡器 IP
ipSource: "remoteAddr"
allow:
  - "10.0.0.50"  # 负载均衡器 IP

# 长期：配置负载均衡器正确传递 X-Forwarded-For
```

### 问题 3：规则不生效

**原因**：
- 插件未正确绑定到路由
- YAML 格式错误

**解决方案**：
```bash
# 检查资源状态
kubectl get edgionplugins -A
kubectl describe edgionplugins <name>

# 检查日志
kubectl logs -n edgion-system <edgion-controller-pod>
```

---

## 测试配置

### 使用 curl 测试

```bash
# 1. 测试白名单
curl -v https://api.example.com/admin
# 应该返回 403 (如果你的 IP 不在白名单)

# 2. 伪造 IP 测试（需要服务器配合）
curl -H "X-Forwarded-For: 192.168.1.100" https://api.example.com/admin
# 如果 192.168.1.100 在白名单，应该成功
```

### 从不同 IP 测试

```bash
# 从办公室测试
curl https://api.example.com/admin

# 从家里测试（应该被拒绝）
curl https://api.example.com/admin

# 从服务器测试
ssh server-in-whitelist
curl https://api.example.com/admin
```

---

## 完整示例

```yaml
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: protected-admin
  namespace: default
spec:
  parentRefs:
    - name: my-gateway
  hostnames:
    - "api.example.com"
  rules:
    # 管理接口：严格 IP 限制
    - matches:
        - path:
            type: PathPrefix
            value: /admin
      filters:
        - type: ExtensionRef
          extensionRef:
            group: edgion.io
            kind: EdgionPlugins
            name: admin-security
      backendRefs:
        - name: admin-service
          port: 8080
    
    # 公开 API：黑名单模式
    - matches:
        - path:
            type: PathPrefix
            value: /api
      filters:
        - type: ExtensionRef
          extensionRef:
            group: edgion.io
            kind: EdgionPlugins
            name: public-security
      backendRefs:
        - name: api-service
          port: 8080
---
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: admin-security
  namespace: default
spec:
  plugins:
    # IP 白名单
    - enable: true
      plugin:
        type: IpRestriction
        config:
          ipSource: "clientIp"
          allow:
            - "10.0.0.0/8"           # 内网
            - "203.0.113.10"         # 办公室 IP 1
            - "203.0.113.20"         # 办公室 IP 2
          deny:
            - "10.0.0.100"           # 排除测试机器
          message: "Admin access restricted to authorized networks"
          status: 403
    
    # 额外认证
    - enable: true
      plugin:
        type: BasicAuth
        config:
          secretRefs:
            - name: admin-users
---
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: public-security
  namespace: default
spec:
  plugins:
    # IP 黑名单
    - enable: true
      plugin:
        type: IpRestriction
        config:
          ipSource: "clientIp"
          deny:
            - "203.0.113.50"         # 恶意 IP
            - "198.51.100.0/24"      # 恶意网段
          defaultAction: "allow"
          message: "Your IP has been blocked due to suspicious activity"
          status: 403
```

**测试场景**：

```bash
# 1. 从办公室访问管理接口 - 成功
curl -u admin:password https://api.example.com/admin/users

# 2. 从家里访问管理接口 - 失败 (IP 不在白名单)
curl https://api.example.com/admin/users
# -> 403 Forbidden

# 3. 恶意 IP 访问公开 API - 失败
curl -H "X-Forwarded-For: 203.0.113.50" https://api.example.com/api/data
# -> 403 Forbidden

# 4. 正常用户访问公开 API - 成功
curl https://api.example.com/api/data
# -> 200 OK
```
