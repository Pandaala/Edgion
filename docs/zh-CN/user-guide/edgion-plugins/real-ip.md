# RealIp 插件

## 概述

RealIp 插件用于从 HTTP 请求头中提取真实的客户端 IP 地址，特别适用于网关部署在 CDN、负载均衡器或其他代理后面的场景。

该插件实现了 Nginx 风格的 IP 提取算法，支持可信代理配置和递归查找。

## 功能特点

- ✅ **可信代理列表** - 支持 CIDR 格式的可信代理配置
- ✅ **多种 Header 支持** - 支持 X-Forwarded-For、X-Real-IP、CF-Connecting-IP 等
- ✅ **递归查找** - Nginx 风格的从右到左遍历算法
- ✅ **IPv4/IPv6 支持** - 完整支持 IPv4 和 IPv6 地址
- ✅ **性能优化** - 使用预编译的 IP Radix Tree 进行快速匹配

## 配置说明

### 基本配置

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: real-ip-basic
  namespace: default
spec:
  requestPlugins:
    - type: RealIp
      config:
        trustedIps:
          - "10.0.0.0/8"
          - "172.16.0.0/12"
          - "192.168.0.0/16"
        realIpHeader: "X-Forwarded-For"
        recursive: true
```

### 配置参数

| 参数 | 类型 | 必填 | 默认值 | 说明 |
|------|------|------|--------|------|
| `trustedIps` | string[] | 是 | - | 可信代理 IP 地址或 CIDR 范围列表 |
| `realIpHeader` | string | 否 | `"X-Forwarded-For"` | 提取真实 IP 的 Header 名称 |
| `recursive` | boolean | 否 | `true` | 是否启用递归查找（Nginx 风格） |

#### trustedIps

可信代理的 IP 地址或 CIDR 范围列表。对应 Nginx 的 `set_real_ip_from` 指令。

- 支持单个 IP 地址：`"192.168.1.1"`
- 支持 CIDR 范围：`"10.0.0.0/8"`
- 支持 IPv6：`"2001:db8::/32"`

#### realIpHeader

指定从哪个 HTTP Header 提取真实 IP。对应 Nginx 的 `real_ip_header` 指令。

常用值：
- `X-Forwarded-For` - 标准代理 Header（默认）
- `X-Real-IP` - Nginx 常用 Header
- `CF-Connecting-IP` - Cloudflare CDN
- `True-Client-IP` - Akamai CDN

#### recursive

启用递归查找模式。对应 Nginx 的 `real_ip_recursive` 指令。

- `true`（默认）：从右到左遍历 Header 中的 IP 列表，找到第一个非可信 IP
- `false`：使用 Header 中最右边的 IP（最后一个代理）

## 工作原理

### 算法流程

```text
1. 检查 client_addr 是否在 trustedIps 中
   ├─ 否 → 直接使用 client_addr 作为 real IP
   └─ 是 → 继续步骤 2

2. 从 realIpHeader 提取 IP 列表
   例如：X-Forwarded-For: "203.0.113.1, 198.51.100.2, 192.168.1.1"

3. 递归查找（如果 recursive=true）
   从右到左遍历：
   ├─ 192.168.1.1 → 在 trustedIps 中 ✓ 继续
   ├─ 198.51.100.2 → 在 trustedIps 中 ✓ 继续
   └─ 203.0.113.1 → 不在 trustedIps 中 ✗ 这是真实 IP！

4. 更新 ctx.request_info.remote_addr（通过 set_remote_addr 方法）
```

### 示例场景

#### 场景 1：典型的 CDN + 负载均衡器

```text
真实客户端: 203.0.113.1
    ↓
CDN (198.51.100.2, trusted)
    ↓ X-Forwarded-For: 203.0.113.1
负载均衡器 (192.168.1.1, trusted)
    ↓ X-Forwarded-For: 203.0.113.1, 198.51.100.2
Edgion Gateway (client_addr: 192.168.1.1)
```

**配置：**
```yaml
trustedIps:
  - "192.168.0.0/16"  # 负载均衡器
  - "198.51.100.0/24" # CDN
realIpHeader: "X-Forwarded-For"
recursive: true
```

**结果：** `remote_addr = 203.0.113.1`

#### 场景 2：Cloudflare CDN

```yaml
trustedIps:
  - "173.245.48.0/20"
  - "103.21.244.0/22"
  - "103.22.200.0/22"
  # ... 更多 Cloudflare IP 范围
realIpHeader: "CF-Connecting-IP"
recursive: false
```

## 使用示例

### 示例 1：基础配置

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: real-ip-basic
  namespace: default
spec:
  requestPlugins:
    - type: RealIp
      config:
        trustedIps:
          - "10.0.0.0/8"
          - "172.16.0.0/12"
          - "192.168.0.0/16"
```

### 示例 2：Cloudflare 配置

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: real-ip-cloudflare
  namespace: default
spec:
  requestPlugins:
    - type: RealIp
      config:
        trustedIps:
          - "173.245.48.0/20"
          - "103.21.244.0/22"
          - "103.22.200.0/22"
          - "103.31.4.0/22"
          - "141.101.64.0/18"
          - "108.162.192.0/18"
          - "190.93.240.0/20"
          - "188.114.96.0/20"
          - "197.234.240.0/22"
          - "198.41.128.0/17"
        realIpHeader: "CF-Connecting-IP"
        recursive: false
```

### 示例 3：多层代理

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: real-ip-multi-tier
  namespace: default
spec:
  requestPlugins:
    - type: RealIp
      config:
        trustedIps:
          - "10.0.0.0/8"       # 内网代理
          - "172.16.0.0/12"    # 私有网络
          - "192.168.0.0/16"   # 本地网络
          - "198.51.100.0/24"  # CDN
        realIpHeader: "X-Forwarded-For"
        recursive: true
```

## 与其他网关对比

| 特性 | Edgion RealIp | Nginx | APISIX | Kong |
|------|---------------|-------|--------|------|
| 可信代理 CIDR | ✅ | ✅ `set_real_ip_from` | ✅ `trusted_addresses` | ⚠️ 简单配置 |
| 自定义 Header | ✅ | ✅ `real_ip_header` | ✅ `source` | ✅ |
| 递归查找 | ✅ | ✅ `real_ip_recursive` | ✅ `recursive` | ❌ |
| 路由级配置 | ✅ | ❌ 仅全局 | ✅ | ❌ |

## 注意事项

1. **安全性**：只信任你控制的代理 IP，不要信任过大的 CIDR 范围
2. **性能**：IP 匹配使用 Radix Tree，性能开销极小
3. **顺序**：插件应该在其他需要使用 `remote_addr` 的插件（如限流、IP 限制）之前执行
4. **全局配置**：如果同时配置了全局 `realIp` 和插件级 `RealIp`，插件配置会覆盖全局配置
5. **实时生效**：插件会直接修改 `ctx.request_info.remote_addr`，后续的插件和访问日志将使用更新后的值

## 常见问题

### Q: 为什么要区分 client_addr 和 remote_addr？

A: 
- `client_addr`: TCP 连接的直接来源 IP（通常是负载均衡器）
- `remote_addr`: 提取后的真实客户端 IP（用于业务逻辑）

### Q: 如何验证配置是否生效？

A: 查看请求日志中的 `remote_addr` 字段，或者在后端服务查看 `X-Real-IP` Header。

### Q: 可以同时信任多个 CDN 吗？

A: 可以，将所有 CDN 的 IP 范围都添加到 `trustedIps` 列表中即可。

### Q: 支持动态更新吗？

A: 是的，更新 EdgionPlugins 资源后，配置会自动热重载。

## 相关资源

- [Nginx ngx_http_realip_module](https://nginx.org/en/docs/http/ngx_http_realip_module.html)
- [APISIX real-ip plugin](https://apisix.apache.org/docs/apisix/plugins/real-ip/)
- [Cloudflare IP Ranges](https://www.cloudflare.com/ips/)
