# 多语言规范

## 语言与目录

| 语言 | 目录 | 状态 |
|------|------|------|
| English | `docs/en/` | 活跃 |
| 中文（简体） | `docs/zh-CN/` | 活跃 |
| 日本語 | `docs/ja/` | 预留，暂未启动 |

## 源语言约定

**zh-CN 为源语言，en 为翻译目标**。

理由：项目核心团队以中文为主，新文档通常先以中文写成。

工作流：
1. 在 zh-CN/ 写完中文文档
2. 翻译到 en/（同一 PR 或后续 PR）
3. 后续修改时，改源语言（zh-CN），再同步翻译

**例外**：如果某篇文档更适合先以英文写成（如面向国际社区的功能），可以以 en 为源，但要在 PR 中注明。

## 对称规则

### 目录结构对称

en/ 和 zh-CN/ 的目录结构必须完全一致：

```
✅ 正确：
docs/en/user-guide/http-route/filters/edgion-plugins/cors.md
docs/zh-CN/user-guide/http-route/filters/edgion-plugins/cors.md

❌ 错误（zh-CN 有但 en 没有）：
docs/zh-CN/user-guide/http-route/backends/circuit-breaker.md
（en/ 下缺少对应文件）
```

### 文件名一致

文件名在所有语言中保持一致（英文 kebab-case），不翻译文件名：

```
✅ docs/zh-CN/getting-started/first-gateway.md
❌ docs/zh-CN/getting-started/第一个网关.md
```

### 章节结构对称

同一文档的章节标题可以翻译，但章节结构（数量、顺序、层级）必须一致。

### YAML 示例一致

YAML 代码块在所有语言中完全相同（包括字段值）。仅注释文字翻译：

```yaml
# zh-CN
# 允许的前端域名
allow_origins: "https://app.example.com"

# en
# Allowed frontend origins
allow_origins: "https://app.example.com"
```

## 术语表

以下术语在中文文档中保持英文原文，不翻译：

### Gateway API 标准术语

| 英文 | 中文文档中的写法 | 不要写成 |
|------|-----------------|---------|
| Gateway | Gateway | 网关（作为资源名时） |
| GatewayClass | GatewayClass | 网关类 |
| HTTPRoute | HTTPRoute | HTTP 路由（作为资源名时） |
| GRPCRoute | GRPCRoute | — |
| TCPRoute | TCPRoute | — |
| UDPRoute | UDPRoute | — |
| TLSRoute | TLSRoute | — |
| Listener | Listener | 监听器（作为字段名时） |
| BackendRef | BackendRef | 后端引用 |
| ParentRef | ParentRef | 父引用 |
| Filter | Filter（作为字段名时） | 过滤器（可用于概念描述） |
| Secret | Secret | 密钥（作为 K8s 资源时） |
| Namespace | Namespace | 命名空间（可用于概念描述） |
| ReferenceGrant | ReferenceGrant | 引用授权 |
| Service | Service（作为 K8s 资源时） | 服务 |

### Edgion 特有术语

| 英文 | 中文文档中的写法 |
|------|-----------------|
| EdgionPlugins | EdgionPlugins |
| EdgionTls | EdgionTls |
| EdgionGatewayConfig | EdgionGatewayConfig |
| Controller | 控制面 / Controller（首次出现标注：控制面（Controller）） |
| Gateway（作为 Edgion 组件） | 数据面 / Gateway（首次出现标注：数据面（Gateway）） |
| edgion-cli | edgion-cli |
| PluginLog | PluginLog |
| access log | access log |

### 通用技术术语

| 英文 | 中文 | 说明 |
|------|------|------|
| TLS termination | TLS 终结 | 可翻译 |
| mTLS | mTLS | 不翻译 |
| load balancing | 负载均衡 | 可翻译 |
| health check | 健康检查 | 可翻译 |
| circuit breaker | 熔断器 | 可翻译 |
| rate limiting | 限流 | 可翻译 |
| session persistence | 会话保持 | 可翻译 |
| upstream | 上游 | 可翻译 |
| downstream | 下游 | 可翻译 |
| backend | 后端 | 可翻译 |

## 同步工作流

### 新增文档

1. 在 zh-CN/ 创建文档（源语言）
2. 翻译到 en/
3. 更新两个目录的 README.md（如果有）
4. 更新 `docs/DIRECTORY.md`

### 修改现有文档

1. 修改 zh-CN/ 中的源文档
2. 同步修改 en/ 中的翻译
3. 如果本次无法同步翻译，在 PR 中标注 `[i18n-sync-needed]`

### 同步状态追踪

当翻译滞后时，可在英文文档顶部加临时标注：

```markdown
> ⚠️ This page may be out of date with the zh-CN version. Last synced: 2026-03-15.
```

任务完成后移除标注。

## 中文排版规则

- 中文与英文/数字之间加空格：`配置 Gateway 的 TLS 终结`
- 中文与行内代码之间加空格：`配置 \`allow_origins\` 字段`
- 使用全角标点：句号用 `。`，逗号用 `，`，冒号用 `：`（代码/YAML 中除外）
- 专有名词使用正确大小写：`Kubernetes`，不是 `kubernetes`
