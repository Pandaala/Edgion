# TODO: 全局 URI 规范化（Path Normalization）

> 状态：**设计阶段待决策**。Edgion 数据面目前缺失 URI 规范化层，导致路径类访问控制可被编码绕过。完整方案涉及数据面架构级改动，需 operator 决策默认开启策略后才能推进实施。

---

## 问题

Edgion 在 URI 规范化层面**完全空白**：

- `session.get_path()` 直接返回 pingora 的 `uri.path()`，percent-encoded 原样保留
- 路由匹配（HTTPRoute `path.type: Exact / PathPrefix`）、所有路径相关插件（`request_restriction`、`csrf` 的路径豁免、`proxy_rewrite` 的 `$uri`、`ctx_set` 模板变量等）拿到的都是 raw 路径
- 无 percent-decode、无 dot-segment 消除、无双斜杠合并、无 `%2F` 处理策略

### 具体后果示例：`request_restriction` 插件路径类 deny 规则可绕过

- **代码路径**：`src/core/gateway/plugins/http/request_restriction/plugin.rs:95` → `src/types/resources/edgion_plugins/plugin_configs/request_restriction.rs:368-401`
- **匹配逻辑**（精确比较 + 正则，无规范化）：
  ```rust
  // get_value
  RestrictionSource::Path => Some(session.get_path().to_string()),
  // matches_deny / matches_allow
  if value == v { return true; }         // 精确字符串比较
  if regex.is_match(value) { return true; }
  ```
- **攻击场景**：
  - 配置 `deny: ["/admin"]`
  - 请求 `/%61dmin` → `get_path()` 返回 `"/%61dmin"` → 与 `"/admin"` 不等 → 通过
  - 上游框架（Spring / Django / Express / FastAPI 等）收到 `/%61dmin` 做 percent-decode → 路由到 `/admin` handler → 绕过成功
- **静态规则难以手动覆盖**的常见变体：

  | 攻击形式 | 解码/规范化后 | 两条静态规则（`/admin`、`/admin/`）能挡？ |
  |---|---|---|
  | `/%61dmin` | `/admin` | ❌ |
  | `/admin%2F` | `/admin/` | ❌ |
  | `/admin/` | `/admin/` | ✅ |
  | `//admin` | `/admin` | ❌ |
  | `/./admin` | `/admin` | ❌ |
  | `/admin/../admin` | `/admin` | ❌ |

  靠 operator 写 regex 覆盖全部变体（`^/(%61|a)(%64|d)(%6d|m)(%69|i)(%6e|n)(/|$|%2[fF])` 这类）已经不可维护。

**该缺口等价于 Envoy 2016-2021 年的"默认不规范化"时代状态**——Envoy 在 CVE-2021-29492 后补了 `path_with_escaped_slashes_action`，Istio 等控制面随后默认打开。

---

## 行业对照

| 产品 | 默认规范化 | 粒度 | 关键机制 |
|---|---|---|---|
| **Nginx** | on | 全局 | `merge_slashes` + URL decoding + 点段消除（默认均启用）。历史背景：IIS Unicode 穿越 CVE-2000-0884 后的 web server 共识 |
| **Apache** | on（部分） | 全局 + per-location | 保留 `AllowEncodedSlashes` 这类 opt-out 旋钮给 CGI 兼容 |
| **Envoy** | **off** | HCM 层（per-listener/filter_chain，不支持 per-route） | `normalize_path` + `merge_slashes` + `path_with_escaped_slashes_action` 四档（KEEP / REJECT / UNESCAPE_AND_REDIRECT / UNESCAPE_AND_FORWARD）。默认 off 是"显式取舍 + 无侵入接管"的设计哲学产物 |
| **Istio**（Envoy 控制面） | **on** | Mesh-wide 配置下沉到各 sidecar HCM | `MeshConfig.pathNormalization` 默认 `BASE`，自 1.11 起。直接响应 2021 年 `%2F` 系列 CVE |
| **Azure Front Door / GCLB / CloudFront** | on | 产品强制 | 不暴露 opt-out（除非走原路径转发的 blob storage 类场景） |
| **Traefik** | off | — | 完全靠 operator 写 regex |
| **APISIX** | off | per-route 插件 | 独立 `uri-normalize` 插件显式启用 |
| **Edgion（当前）** | **none** | — | 什么都没有，连 Traefik 的"靠 operator regex"门槛都没明示 |

**取舍维度**：
- Nginx / Istio / CDN 类产品选默认 on——安全基座优先
- Envoy / Traefik / APISIX 选默认 off 或插件化——无侵入优先，承担 operator 踩坑成本

---

## 决策点

**核心问题：Edgion 走 Nginx/Istio 默认 on 路线，还是 Envoy/Traefik 默认 off 路线？**

几个辅助判断：
- Edgion 原生定位是"代理 + 安全层"（插件链、Gateway API ACL）——偏向 Nginx/Istio 阵营
- Edgion 新建项目，没有 2016 年 Envoy 的"无侵入接管现有流量"包袱
- 插件生态已经大量基于路径做决策（`request_restriction`、`csrf`、`ip_restriction` 的路径豁免等）——默认 off 意味着每个插件都要单独想规范化问题

**倾向性结论（待 operator 决策）**：默认 **on**，对标 Istio；但提供 per-Gateway opt-out + `%2F` 处理策略配置，对应 Envoy 的四档选项。

### 待决策字段

1. **默认值**：on / off？
2. **配置层级**：
   - 顶层 `EdgionGatewayConfig`（全局默认）？
   - `Gateway` 资源 spec（per-Gateway 覆盖）？
   - `Listener` 粒度（per-listener 覆盖）？
   - 三级继承（`config < Gateway < Listener`）？
3. **规范化项**（可独立开关或统一）：
   - `normalize_percent`：percent-decode unreserved 字符
   - `merge_slashes`：`//` → `/`
   - `remove_dot_segments`：`/./`、`/../` 消除
   - `escaped_slash_action`：`%2F` 处理（KEEP / REJECT_403 / UNESCAPE_REDIRECT / UNESCAPE_FORWARD，对齐 Envoy）
   - `case_normalization`：path 是否 lowercase？（可选，默认 off；某些 backend 大小写敏感）
4. **作用点**：
   - 入口（pingora 接受请求后立刻）→ 所有后续逻辑看规范化路径
   - 仅在路由匹配之前 → 转发给上游时还原 raw？
   - 只改 session 暴露的 `get_path()` 视图？
5. **上游转发策略**：
   - `forward_raw_path: true`（默认，保留上游契约）：`get_path()` 返回规范化路径用于匹配，但 pingora 转发时发原始路径
   - `forward_normalized_path`：用规范化路径转发（更安全但可能破坏依赖 raw 的上游，如某些把 `%2F` 当字面量 slash 而非分隔的 API）
6. **HTTPRoute 路由匹配语义变更**：
   - `path.type: Exact` 用 raw 还是 normalized 比较？
   - `PathPrefix` 前缀匹配是否在规范化后进行？
   - 这个变更会影响现有 HTTPRoute 配置的匹配行为，需要迁移/回归评估

### 依赖 / 关联影响面

触发规范化后**所有路径相关组件**都需评估：

- **路由层**：`HTTPRoute` / `TLSRoute` / `GRPCRoute` 的 path matcher
- **插件**：
  - `request_restriction`（最直接受益）
  - `csrf`（token 校验的路径豁免）
  - `proxy_rewrite`（`$uri` 模板变量、path rewrite）
  - `ip_restriction` / `ForwardAuth`（有些场景基于 path 决策）
  - `ctx_set` 的 `$uri` / `$original_uri` 变量——可能需要区分 raw 和 normalized 两个变量
  - `rate_limit` / `rate_limit_redis`（key 可包含 path）
  - `access_log`（记录 raw 还是 normalized？两者都要）
- **observability**：
  - `tracing` span 的 `http.url` / `http.target`
  - Prometheus `http_requests_total{path=...}`（cardinality 风险不变，但取值来源变了）
- **集成测试**：所有 `examples/test/conf/` 下涉及路径编码的用例

---

## 参考 CVE / 历史事件

- **CVE-2000-0884**：Microsoft IIS Unicode directory traversal（触发 Nginx/Apache 等默认 on 规范化）
- **CVE-2021-29492**：Envoy path_with_escaped_slashes 处理缺口（推动 Envoy 新增 action 字段 + Istio 默认 on）
- **CVE-2022-30065 / CVE-2022-35202**（各种网关 `..;/` 绕过）：规范化必须处理分号、逗号等分隔符变体
- **Orange Tsai 系列 research**（Black Hat 演讲 "A New Attack Surface on MS Exchange"、"Breaking Parser Logic"）：多层 URL 解析不一致导致的安全漏洞经典案例

---

## 工作量估算

完整实现 + 测试 + 文档：**1.5–2 周**
- 核心规范化逻辑：2–3 天
- 各插件适配与 `raw` / `normalized` 双视图：3–4 天
- HTTPRoute 路由匹配语义调整 + 迁移评估：2 天
- 集成测试扩充（编码变体覆盖）：2 天
- 文档 + 迁移指南：1 天

---

## 非目标（本项目**不**做）

- HTTP/2 / HTTP/3 路径处理差异（pingora 已统一到 HTTP/1.1 语义视图）
- 查询字符串规范化（`?a=1&a=2` 语义、`+` vs `%20` 处理）——独立问题，单独立项
- 请求体 / Header 的 percent-encoding 处理（只关心 path）

---

## 中间态建议（本项目上线前）

- **operator 侧**：路径类黑名单用 `deny_regex`（正则）而非 `deny`（字符串精确），并覆盖常见编码变体——至少包含 `%61`（`a`）、`%2F`（`/`）、`/./`、`//` 等
- **新用户配置审查**：review 配置时重点看是否存在"精确字符串 path deny / allow"规则，若有应要求改用 regex 直到本项目落地

---

## 下一步

1. Operator 决策"默认 on / off"
2. 基于决策起草详细设计 doc 到 `docs/superpowers/specs/`
3. 设计 review 通过后进入实施
4. 实施完成后：
   - 关闭本 todo
   - 在 `skills/04-review/uri-normalization/` 沉淀判定规则（下次 review 再报"路径未规范化"时的关闭标准）
