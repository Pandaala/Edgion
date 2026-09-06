# RealIp 插件

> **🔌 Edgion 扩展**
> 
> RealIp 是 `EdgionPlugins` CRD 提供的真实客户端 IP 提取插件，不属于标准 Gateway API。

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
  namespace: production
spec:
  requestPlugins:
    - type: RealIp
      config:
        trustedIps:
          - name: private-ranges
            cidrs:
              - "10.0.0.0/8"
              - "172.16.0.0/12"
              - "192.168.0.0/16"
        realIpHeader: "X-Forwarded-For"
        recursive: true
```

### 配置参数

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| `trustedIps` | Array\<IpGroup\> | —（必填） | 可信代理 IP 分组列表 |
| `realIpHeader` | String | `"X-Forwarded-For"` | 提取真实 IP 的 Header 名称 |
| `recursive` | Boolean | `true` | 是否启用递归查找（Nginx 风格） |
| `maxTrustedHops` | Integer | 无（不限） | `recursive=true` 时可信代理跳数上限，详见 [安全取舍](#安全取舍trustedips-和-maxtrustedhops) |

#### trustedIps

可信代理 IP 分组列表。对应 Nginx 的 `set_real_ip_from` 指令。

列表中每一项都是一个 `IpGroup` 对象：

| 字段 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `name` | String | 是 | 分组名称；≤63 字符，字符集 `[a-zA-Z0-9._-]`，首尾必须是字母或数字，在列表内唯一 |
| `description` | String | 否 | 分组的可选描述 |
| `cidrs` | Array\<String\> | 是 | 非空的 IP 地址或 CIDR 范围列表 |

`cidrs` 中每一项支持：

- 单个 IP 地址：`"192.168.1.1"`
- CIDR 范围：`"10.0.0.0/8"`
- IPv6：`"2001:db8::/32"`

#### realIpHeader

指定从哪个 HTTP Header 提取真实 IP。对应 Nginx 的 `real_ip_header` 指令。

常用值：
- `X-Forwarded-For` - 标准代理 Header（默认）
- `X-Real-IP` - Nginx 常用 Header
- `CF-Connecting-IP` - Cloudflare CDN
- `True-Client-IP` - Akamai CDN

该名称必须是合法的 HTTP header 字段名（RFC 9110 `token` 字符集：字母、数字以及
`!#$%&'*+-.^_`|~`），且不超过 256 字节。`"X Forwarded For"` 这类拼写错误在运行时
永远匹配不到任何已解析的 header，若不校验就会静默地让真实 IP 提取失效、把
`RemoteIp` 固定为可信代理地址。因此非法名称在准入阶段即被报告：资源会被标记为
`Accepted=False` 且 `reason=Invalid`。

**插件路径为 fail-closed。** 对 `EdgionPlugins` 而言，非法的 RealIp 配置会把插件链
封闭：命中该资源的每个请求都返回 `500 plugin configuration error`，而不是带着失效的
提取逻辑放行。修正 header 名称后流量恢复。全局
`EdgionGatewayConfig.spec.realIp` 路径不封闭：Gateway 记录一条 error 日志，并按
`realIp` 未配置的方式运行（不做真实 IP 提取）。

#### recursive

启用递归查找模式。对应 Nginx 的 `real_ip_recursive` 指令。

- `true`（默认）：从右到左遍历 Header 中的 IP 列表，找到第一个非可信 IP
- `false`：使用 Header 中最右边的 IP（最后一个代理）

#### maxTrustedHops

`recursive=true` 时可信代理跳数上限（可选）。对应 Envoy 的 `xff_num_trusted_hops` 概念。

- `null`（默认）：无上限，Nginx 兼容行为——从右到左遍历直到遇到首个非可信 IP
- `N`（>0）：右→左遍历中最多跳过 `N` 个可信 IP；如仍未遇到非可信 IP，返回第 `N` 个位置的 IP（即便其可信）

**什么时候应该设置？**

`maxTrustedHops` 是**辅助防线**，用来对抗"`trustedIps` 配得过宽 + 攻击者能从该范围内发起请求"的情景（见下方[安全取舍](#安全取舍trustedips-和-maxtrustedhops)）。推荐生产部署显式设置为**实际代理链层数**（如 CDN → LB → Gateway 为 `2`）。

## 安全取舍：trustedIps 和 maxTrustedHops

### 核心威胁：宽信任 CIDR 下的 XFF 深度伪造

`recursive=true` 模式下，默认无跳数上限。若 `trustedIps` 配得过宽（例如整个 `10.0.0.0/8`），且攻击者能从该范围内的 IP 发起请求：

```text
配置: trustedIps: ["10.0.0.0/8"], recursive: true
攻击者从 10.0.0.1 发送:
  X-Forwarded-For: 1.2.3.4, 10.0.0.99, 10.0.0.50

右→左遍历:
  10.0.0.50 → 可信 → 继续
  10.0.0.99 → 可信 → 继续
  1.2.3.4   → 不可信 → 返回

结果：网关解析出的 `RemoteIp = 1.2.3.4`（攻击者伪造）
```

攻击者随后：
- 绕过依赖 `RemoteIp` 的 IP 白名单（`ip_restriction`）
- 把基于 IP 的限流计数写到任意其他用户名下
- 让审计日志记录伪造的源 IP

### 两道防线（需同时配置）

**第一道（主要）：精确配置 `trustedIps`**

`trustedIps` **只应包含真实代理出口 IP**，不应为了"偷懒"覆盖整个内网段：

```yaml
# ❌ 反例：覆盖面过宽，任何来自内网的请求都被当作可信代理
trustedIps:
  - name: internal
    cidrs:
      - "10.0.0.0/8"

# ✅ 正例：仅包含实际代理（如 Cloudflare 回源 IP、内部 LB IP）
trustedIps:
  - name: cloudflare
    cidrs:
      - "103.21.244.0/22"
      - "103.22.200.0/22"
      - "173.245.48.0/20"
  - name: internal-lb
    description: Internal LB egress (single address)
    cidrs:
      - "10.1.0.10"
```

**第二道（辅助）：设置 `maxTrustedHops` 作为硬上限**

即便第一道防线可能配置错误，设置 `maxTrustedHops = 实际代理链层数`，限制攻击者能伪造的 XFF 前缀长度：

```yaml
trustedIps:
  - name: cloudflare
    cidrs:
      - "103.21.244.0/22"   # Cloudflare（1 跳）
  - name: internal-lb
    cidrs:
      - "10.1.0.10"         # 内部 LB（1 跳）
maxTrustedHops: 2         # 只信任 2 跳；攻击者无法通过填充多层"可信" IP 推进到左端
```

### 何时可不设 `maxTrustedHops`

- `recursive: false`：无效果（非递归模式只看最右 IP）
- `trustedIps` 已严格收敛到固定几个 IP / 小 CIDR 且可审计
- 代理链动态变化（如服务网格随节点扩缩容），难以确定固定跳数——此时只能靠第一道防线

### 与 Nginx / Envoy 对比

| 行为 | Edgion | Nginx | Envoy |
|------|--------|-------|-------|
| 递归遍历 | `recursive: true` | `real_ip_recursive on` | —（隐含） |
| 跳数上限 | `maxTrustedHops`（opt-in） | ❌ 无 | `xff_num_trusted_hops`（必填） |
| 默认 | 无上限（Nginx 兼容） | 无上限 | 0 |

Edgion 默认采用 Nginx 风格（无上限，向后兼容），同时通过 `maxTrustedHops` 为 Envoy 熟悉的 operator 和安全敏感部署提供硬边界。

## 工作原理

### 算法流程

```text
1. 检查 downstream_peer_ip 是否在 trustedIps 中
   ├─ 否 → 保持 downstream_peer_ip 作为 RemoteIp
   └─ 是 → 继续步骤 2

2. 从 realIpHeader 提取 IP 列表
   例如：X-Forwarded-For: "203.0.113.1, 198.51.100.2, 192.168.1.1"

3. 递归查找（如果 recursive=true）
   从右到左遍历：
   ├─ 192.168.1.1 → 在 trustedIps 中 ✓ 继续
   ├─ 198.51.100.2 → 在 trustedIps 中 ✓ 继续
   └─ 203.0.113.1 → 不在 trustedIps 中 ✗ 这是真实 IP！

4. 更新 ctx.request_info.remote_ip（通过 set_remote_ip 方法）
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
Edgion Gateway (downstream_peer_ip: 192.168.1.1)
```

**配置：**
```yaml
trustedIps:
  - name: load-balancer
    cidrs:
      - "192.168.0.0/16"
  - name: cdn
    cidrs:
      - "198.51.100.0/24"
realIpHeader: "X-Forwarded-For"
recursive: true
```

**结果：** `remote_ip = 203.0.113.1`

#### 场景 2：Cloudflare CDN

```yaml
trustedIps:
  - name: cloudflare
    cidrs:
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
  namespace: production
spec:
  requestPlugins:
    - type: RealIp
      config:
        trustedIps:
          - name: private-ranges
            cidrs:
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
  namespace: production
spec:
  requestPlugins:
    - type: RealIp
      config:
        trustedIps:
          - name: cloudflare
            cidrs:
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
  namespace: production
spec:
  requestPlugins:
    - type: RealIp
      config:
        trustedIps:
          - name: internal
            description: Internal proxy and private networks
            cidrs:
              - "10.0.0.0/8"
              - "172.16.0.0/12"
              - "192.168.0.0/16"
          - name: cdn
            cidrs:
              - "198.51.100.0/24"
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
3. **顺序**：插件应该在其他需要使用 `remote_ip` 的插件（如限流、IP 限制）之前执行
4. **全局配置**：如果同时配置了全局 `realIp` 和插件级 `RealIp`，插件配置会覆盖全局配置
5. **实时生效**：插件会直接修改 `ctx.request_info.remote_ip`，后续的插件和访问日志将使用更新后的值

## 常见问题

### Q: 为什么要区分 DirectPeerIp 和 RemoteIp？

A: 
- `DirectPeerIp`：传输连接的物理来源 IP（通常是负载均衡器）
- `RemoteIp`：经过可信 PROXY/RealIp 边界解析的客户端身份（用于业务逻辑）

### Q: 如何验证配置是否生效？

A：查看请求日志中的 `remote_ip` 字段，或者在后端服务查看配置的 RemoteIp Header（默认是 `X-Real-IP`）。

### Q: 可以同时信任多个 CDN 吗？

A: 可以，将每个 CDN 作为 `trustedIps` 下独立的一个分组（每个 CDN 一个 `IpGroup`，把它的范围写进 `cidrs`）。

### Q: 支持动态更新吗？

A: 是的，更新 EdgionPlugins 资源后，配置会自动热重载。

## 相关文档

- [IP 限制插件](../http-route/filters/edgion-plugins/ip-restriction.md) - IP 黑白名单（常与 RealIp 配合使用）
- [HTTPRoute 总览](../http-route/overview.md) - HTTPRoute 配置总览
- [Nginx ngx_http_realip_module](https://nginx.org/en/docs/http/ngx_http_realip_module.html) - Nginx 真实 IP 模块参考
- [Cloudflare IP Ranges](https://www.cloudflare.com/ips/) - Cloudflare CDN IP 范围
