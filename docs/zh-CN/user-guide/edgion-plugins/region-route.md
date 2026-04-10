# RegionRoute 插件

## 概述

RegionRoute 使用 0–1000 一致性哈希范围模型将流量路由到多个 region，适用于同一服务在多个 region 主主部署、需要按 account/user 等维度做确定性粘性路由的场景。

**路由优先级（从高到低）：**
1. `direct_region` — 从请求 header/cookie 显式指定目标 region（可选）
2. `region_key_hash` — 从 header/cookie 读取预计算的 0–1000 整数，跳过 hash 计算
3. `region_key` — 提取 key，计算 SHA-256 mod 1001，映射到 region 范围

`region_key` 为空或请求中取不到 key 时，插件直接放行，不修改上游目标。

**核心行为：**
- 只改变上游 **IP**（通过 DNS 解析 `backend_endpoint`）。原始请求的 **Host header**、**SNI**、路径、body 全部保持不变。
- 每个 region 占用 0–1000 中的一段范围，范围不得重叠，允许有间隙。
- 可选的 auto failover 通过 DNS 探测每个 region 的健康状态；若某 region DNS 无返回，流量自动切到最健康的其他 region。

## 配置

```yaml
apiVersion: edgion.io/v1
kind: EdgionPlugins
metadata:
  name: my-region-route
  namespace: default
spec:
  requestPlugins:
    - type: RegionRoute
      config:
        # region 定义 — hash 范围 [low, high]，闭区间，0–1000
        regions:
          - name: virginia
            hashRange: [0, 600]          # 60% 流量
            backendEndpoint: virginia-internal.example.com:8443
            tls: true
          - name: ohio
            hashRange: [601, 1000]       # 40% 流量
            backendEndpoint: ohio-internal.example.com:8443
            tls: true

        # 可选：服务可通过 header/cookie 强制指定目标 region
        directRegion:
          keyFrom:
            - type: header
              name: X-Direct-Region      # 值必须是某个 region 的 name

        # 可选：预计算的 0–1000 整数（避免每次请求重新 hash）
        regionKeyHash:
          keyFrom:
            - type: cookie
              name: x_region_hash        # 如 Set-Cookie: x_region_hash=342

        # hash 计算用的 key，按顺序取第一个非空值
        # 空列表 = 插件 no-op
        regionKey:
          - type: header
            name: X-Account-Id
          - type: cookie
            name: account_id
          # - type: clientIp            # 按需开启 IP fallback

        # 可选：auto failover（基于 DNS 探测）
        autoFailover:
          enabled: true
          healthyThreshold: 0.5         # DNS 无返回时触发
          probeIntervalMs: 5000         # 每 5s 探测一次
          notifyHeader: X-Failover-Region   # 写入响应 header（便于 debug）
          onAllRegionsDown: pass        # pass | reject（返回 503）
```

## Region 哈希范围

0–1000 范围让你可以直观地控制流量比例：

| 意图 | virginia 范围 | ohio 范围 |
|------|--------------|----------|
| 各 50% | `[0, 500]` | `[501, 1000]` |
| 60 / 40 | `[0, 600]` | `[601, 1000]` |
| 80 / 20 | `[0, 800]` | `[801, 1000]` |
| 灰度（ohio=5%） | `[0, 950]` | `[951, 1000]` |

允许有间隙，落在间隙的请求放行到原始上游。

## 预计算 Hash（regionKeyHash）

后端服务可以一次性计算 hash 并存入 cookie，后续请求网关直接读取，跳过 SHA-256：

```python
# Python 示例 — 与网关算法一致
import hashlib, struct
digest = hashlib.sha256(account_id.encode()).digest()
h = struct.unpack(">Q", digest[:8])[0]
region_hash = h % 1001
# 写入响应: Set-Cookie: x_region_hash=<region_hash>
```

## Direct Region Override

服务可在请求中设置 `X-Direct-Region` 强制跳转到指定 region：

```http
GET /api/data HTTP/1.1
Host: api.example.com
X-Account-Id: acct-123
X-Direct-Region: ohio
```

若指定 region 不健康，插件会转入 auto failover（如已配置），不会强制连到不健康的后端。

## Access Log 字段

| 日志条目 | 含义 |
|---------|------|
| `direct=ohio(from:hdr(X-Direct-Region));` | 从 header 读到 direct override，路由到 ohio |
| `hash_pass=342 from:cookie(x_region_hash);` | 从 cookie 读到预计算 hash |
| `key=hdr(X-Account-Id) hash=342;` | 从 header 计算得到 hash（不记录原始值） |
| `→virginia virginia-internal.example.com:8443 sni=api.example.com;` | 实际跳转目标 |
| `failover→ohio (100%);` | 自动 failover 到 ohio |
| `region_key missing, pass;` | 取不到 key，放行 |

## 注意事项

- `backend_endpoint` 每次请求都会 DNS 解析（复用 Pingora 的 `ExternalJumpPreset` 机制），建议使用内部稳定 DNS 名指向 region 负载均衡器。
- 插件**不修改** Host header 和 SNI。若上游校验 SNI，需确保后端证书覆盖原始 hostname。
- Auto failover 仅基于 DNS 活性探测（DNS 是否返回 IP），不做应用层健康检查。
- `regionKey: []`（空列表）使插件变为 no-op，可用于分阶段上线。
