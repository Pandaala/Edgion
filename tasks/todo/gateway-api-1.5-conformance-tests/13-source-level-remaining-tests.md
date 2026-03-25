# Conformance Tests - 源码级精确测试用例 (续)

## HTTPRouteExactPathMatching (6 个用例)

**Route**: `exact-matching`, **Gateway**: `same-namespace`

| # | Request Path | Expected Backend | Expected Status | 说明 |
|---|-------------|-----------------|----------------|------|
| 1 | `/one` | `infra-backend-v1` | 200 | Exact 匹配 |
| 2 | `/two` | `infra-backend-v2` | 200 | Exact 匹配 |
| 3 | `/` | - | 404 | 不匹配 |
| 4 | `/one/example` | - | 404 | 路径更长，不匹配 |
| 5 | `/two/` | - | 404 | 尾部斜杠不匹配 |
| 6 | `/Two` | - | 404 | **大小写敏感** |

---

## HTTPRouteHostnameIntersection (31 个用例)

### Group 1: 有交集的 hostname

| # | Host | Path | Expected Backend | Expected Status | 说明 |
|---|------|------|-----------------|----------------|------|
| 1 | `very.specific.com` | `/s1` | `infra-backend-v1` | 200 | 精确匹配 |
| 2 | `very.specific.com:1234` | `/s1` | `infra-backend-v1` | 200 | 带端口也匹配 |
| 3 | `non.matching.com` | `/s1` | - | 404 | Host 不匹配 |
| 4 | `foo.nonmatchingwildcard.io` | `/s1` | - | 404 | 不匹配的通配符 |
| 5 | `foo.wildcard.io` | `/s1` | - | 404 | 通配符不匹配此规则 |
| 6 | `very.specific.com` | `/non-matching-prefix` | - | 404 | 路径不匹配 |
| 7 | `foo.wildcard.io` | `/s2` | `infra-backend-v2` | 200 | 通配符匹配 |
| 8 | `bar.wildcard.io` | `/s2` | `infra-backend-v2` | 200 | 通配符匹配 |
| 9 | `foo.bar.wildcard.io` | `/s2` | `infra-backend-v2` | 200 | 多级子域通配符匹配 |
| 10 | `non.matching.com` | `/s2` | - | 404 | 不匹配 |
| 11 | `wildcard.io` | `/s2` | - | 404 | 裸域不匹配 `*.wildcard.io` |
| 12 | `very.specific.com` | `/s2` | - | 404 | 精确域不匹配通配符规则 |
| 13 | `foo.wildcard.io` | `/non-matching-prefix` | - | 404 | 路径不匹配 |
| 14 | `very.specific.com` | `/s3` | `infra-backend-v3` | 200 | listener+route 交集 |
| 15 | `non.matching.com` | `/s3` | - | 404 | |
| 16 | `foo.specific.com` | `/s3` | - | 404 | 子域不匹配精确域 |
| 17 | `foo.wildcard.io` | `/s3` | - | 404 | |
| 18 | `very.specific.com` | `/non-matching-prefix` | - | 404 | |
| 19 | `foo.anotherwildcard.io` | `/s4` | `infra-backend-v1` | 200 | 另一个通配符 |
| 20 | `bar.anotherwildcard.io` | `/s4` | `infra-backend-v1` | 200 | |
| 21 | `foo.bar.anotherwildcard.io` | `/s4` | `infra-backend-v1` | 200 | |
| 22 | `anotherwildcard.io` | `/s4` | - | 404 | 裸域 |
| 23 | `foo.wildcard.io` | `/s4` | - | 404 | 不同通配符 |
| 24 | `very.specific.com` | `/s4` | - | 404 | |
| 25 | `foo.anotherwildcard.io` | `/non-matching-prefix` | - | 404 | |

### Group 2: 无交集
| 26 | `specific.but.wrong.com` | `/s5` | - | 404 | listener 和 route hostname 无交集 |
| 27 | `wildcard.io` | `/s5` | - | 404 | |

### Group 3: 无 hostname 的 listener
| 28 | `first.com` | `/` | `infra-backend-v2` | 200 | 无 hostname listener 匹配所有 |
| 29 | `sub.first.com` | `/` | `infra-backend-v2` | 200 | |
| 30 | `second.com` | `/` | `infra-backend-v2` | 200 | |
| 31 | `third.com` | `/` | - | 404 | route 有 hostname 限制 |

---

## HTTPRouteWeight

**Route**: `weighted-backends`, **Gateway**: `same-namespace`

| Backend | Weight | 期望流量比例 |
|---------|--------|------------|
| `infra-backend-v1` | 70 | ~70% |
| `infra-backend-v2` | 30 | ~30% |
| `infra-backend-v3` | 0 | 0% (不应收到流量) |

**验证**: 使用 `weight.TestWeightedDistribution()` 统计多次请求的分布

---

## HTTPRouteMethodMatching (~15 个用例)

**Route**: `method-matching`, **Gateway**: `same-namespace`

| # | Method | Path | Headers | Expected Backend | Expected Status | 说明 |
|---|--------|------|---------|-----------------|----------------|------|
| 1 | POST | `/` | - | `infra-backend-v1` | 200 | 基本方法匹配 |
| 2 | GET | `/` | - | `infra-backend-v2` | 200 | 基本方法匹配 |
| 3 | HEAD | `/` | - | - | 404 | 未配置的方法 |
| 4 | POST | `/path` | - | backend | 200 | 方法+路径组合 |
| 5 | GET | `/path` | `Version: one` | backend | 200 | 方法+路径+header AND |
| 6 | POST | `/path` | `Version: one` | - | 404 | AND 条件不全满足 |
| 7+ | 各种 OR/AND 组合 | | | | | 多条件匹配优先级 |

**关键点**:
- 方法匹配是精确的
- 多个 match 之间是 OR 关系
- 同一 match 内的条件是 AND 关系
- 按 Gateway API spec 定义的优先级排序

---

## HTTPRouteRequestHeaderModifier (7 个用例)

**Route**: `request-header-modifier`, **Gateway**: `same-namespace`

| # | Path | Request Headers | Expected Backend Headers | Absent Headers | 说明 |
|---|------|----------------|------------------------|---------------|------|
| 1 | `/set` | `Some-Other-Header: val` | `Some-Other-Header: val`, `X-Header-Set: set-overwrites-values` | - | set 新 header |
| 2 | `/set` | `Some-Other-Header: val`, `X-Header-Set: some-other-value` | `X-Header-Set: set-overwrites-values` | - | set 覆盖已有 |
| 3 | `/add` | `Some-Other-Header: val` | `X-Header-Add: add-appends-values` | - | add 新 header |
| 4 | `/add` | `X-Header-Add: some-other-value` | `X-Header-Add: some-other-value,add-appends-values` | - | add 追加到已有值 |
| 5 | `/remove` | `X-Header-Remove: val` | - | `X-Header-Remove` | remove 删除 header |
| 6 | `/multiple` | `X-Header-Set-2: set-val-2, X-Header-Add-2: add-val-2, X-Header-Remove-2: remove-val-2, Another-Header: another-header-val` | `X-Header-Set-1: header-set-1, X-Header-Set-2: header-set-2, X-Header-Add-1: header-add-1, X-Header-Add-2: add-val-2,header-add-2, X-Header-Add-3: header-add-3, Another-Header: another-header-val` | `X-Header-Remove-1, X-Header-Remove-2` | 多操作组合 |
| 7 | `/case-insensitivity` | `x-header-set: original-val-set, x-header-add: original-val-add, x-header-remove: original-val-remove` | `X-Header-Set: header-set, X-Header-Add: original-val-add,header-add` | `x-header-remove, X-Header-Remove` | **大小写不敏感** |

---

## HTTPRouteResponseHeaderModifier (8 个用例)

| # | Path | Backend Response Headers | Expected Client Response Headers | Absent in Response | 说明 |
|---|------|------------------------|-------------------------------|-------------------|------|
| 1 | `/set` | `Some-Other-Header: val` | `X-Header-Set: set-overwrites-values, Some-Other-Header: val` | - | set |
| 2 | `/set` (已有) | `X-Header-Set: some-other-value, Some-Other-Header: val` | `X-Header-Set: set-overwrites-values` | - | set 覆盖 |
| 3 | `/add` | `Some-Other-Header: val` | `X-Header-Add: add-appends-values, Some-Other-Header: val` | - | add |
| 4 | `/add` (已有) | `X-Header-Add: some-other-value` | `X-Header-Add: some-other-value,add-appends-values` | - | add 追加 |
| 5 | `/remove` | `X-Header-Remove: val` | - | `X-Header-Remove` | remove |
| 6 | `/multiple` | 多个 header | `X-Header-Set-1/2, X-Header-Add-1/2/3, Another-Header` | `X-Header-Remove-1, X-Header-Remove-2` | 组合 |
| 7 | `/case-insensitivity` | - | 大小写不敏感处理 | `x-header-remove, X-Header-Remove` | 大小写 |
| 8 | `/response-and-request-header-modifiers` | 多个 header | 同时验证 request 和 response header 修改 | `X-Header-Remove-1/2` | 双向修改 |

**用例 8 特殊**: 同时配置 RequestHeaderModifier + ResponseHeaderModifier，验证两者同时工作

---

## HTTPRouteRedirectHostAndStatus (Source 确认)

| # | Path | Expected Status | Redirect Host | 说明 |
|---|------|----------------|--------------|------|
| 1 | `/` | 302 | `example.com` | 默认 302 |
| 2 | `/` | 301 | `example.org` | 自定义 301 |

---

## HTTPRouteRedirectScheme

| # | Path | Request Scheme | Expected Status | Redirect Scheme | Redirect Port |
|---|------|---------------|----------------|----------------|--------------|
| 1 | `/scheme-redirect` | http | 302 | `https` | - |

---

## HTTPRouteRedirectPort

| # | Path | Expected Status | Redirect Port |
|---|------|----------------|--------------|
| 1 | `/port-redirect` | 302 | `8443` |

---

## HTTPRouteRedirectPortAndScheme

需要 `GatewayPort8080` feature.

| # | Path | Expected Status | Redirect Scheme | Redirect Port |
|---|------|----------------|----------------|--------------|
| 1 | `/port-and-scheme-redirect` | 302 | `https` | `443` |
