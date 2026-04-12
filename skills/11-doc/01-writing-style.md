# 写作风格规范

## 人称与语气

| 规则 | 正确 | 错误 |
|------|------|------|
| 用"你"称呼读者 | "你可以通过…" | "用户可以通过…" |
| 用主动语态 | "Gateway 解密 TLS 连接" | "TLS 连接被 Gateway 解密" |
| 直接、简洁 | "配置 `allow_origins`" | "你需要去配置 `allow_origins` 这个字段" |
| 不加无意义修饰 | "创建 Secret" | "请创建一个 Secret 资源" |

**语气**：专业但不生硬。像同事间的技术交流，不是产品说明书。

## 标题规则

### 一级标题

- 就是功能/资源/概念的名称
- 不加冗余前缀

| 正确 | 错误 |
|------|------|
| `# CORS 插件` | `# Edgion CORS 插件使用指南` |
| `# TLS 终结` | `# 关于 TLS 终结的配置说明` |
| `# HTTPRoute 总览` | `# HTTPRoute 用户指南总览文档` |

### 章节标题

- 用动词或名词短语，不用完整句子
- 按功能含义命名，不按文档位置命名

| 正确 | 错误 |
|------|------|
| `## 快速开始` | `## 第一节：快速开始` |
| `## 配置参数` | `## 本章介绍配置参数` |
| `## 常见问题` | `## FAQ 部分` |

## 开篇模式

### Edgion 扩展功能标注

属于 Edgion 扩展（非标准 Gateway API）的功能，在一级标题后立即用 blockquote 标注：

```markdown
# CORS 插件

> **🔌 Edgion 扩展**
> 
> CORS 是 `EdgionPlugins` CRD 提供的跨域配置插件，不属于标准 Gateway API。
```

标注格式固定为：
- 首行：`> **🔌 Edgion 扩展**`
- 空行：`> `
- 说明行：一句话说明这是什么 CRD / 什么能力，以及它不属于标准 Gateway API

**不需要标注的**：标准 Gateway API 功能（HTTPRoute、Gateway、GatewayClass 等）。

### 概述段落

标注之后（或无标注时在一级标题后），用 1-3 句话说明：
1. 这个功能是什么
2. 解决什么问题

不要在概述段落讲配置细节。

## 技术术语

### 不翻译的术语

以下 Gateway API 标准术语在中文文档中保持英文：

- Gateway, GatewayClass, HTTPRoute, GRPCRoute, TCPRoute, UDPRoute, TLSRoute
- Listener, BackendRef, ParentRef, SectionName
- Secret, Namespace, ConfigMap, ReferenceGrant
- Filter, Match, Rule
- Endpoint, Service, Pod

### 首次出现时标注

中文文档中，Edgion 特有的概念首次出现时用括号标注英文原文：

```markdown
控制面（Controller）负责 Kubernetes 资源处理，数据面（Gateway）负责流量代理。
```

后续出现直接用中文即可。

### 代码/配置中的名称

YAML 字段名、CLI 参数、环境变量等始终用原文加反引号：

```markdown
配置 `allow_origins` 字段来指定允许的来源域名。
```

## 内容边界

### docs/ 中可以讲的

- 功能是什么、解决什么问题
- 怎么配置（YAML 示例）
- 配置参数说明
- 使用场景和最佳实践
- 常见问题和排错
- 与其他功能的关联

### docs/ 中不要讲的

- 内部实现细节（Handler 流程、数据结构、源码位置）
- 代码层面的设计决策
- 性能实现机制
- 具体的 Rust 代码示例（dev-guide 例外）

如果确实需要涉及内部知识，链接到 dev-guide/ 的对应文档。

## 格式约定

### 强调

| 用途 | 格式 | 示例 |
|------|------|------|
| 代码、配置字段、CLI 命令 | 反引号 | `allow_origins`、`kubectl apply` |
| 重要警告 | 粗体 | **不要在生产环境使用** |
| 功能名/产品名 | 普通文字 | Gateway API、Edgion |
| 安全警告 | ⚠️ + 粗体 | ⚠️ **仅用于开发环境** |
| 新概念定义 | 粗体 | **TLS 终结**是指… |

### 列表

- 步骤用有序列表（1. 2. 3.）
- 枚举选项用无序列表（-）
- 列表项以句号结尾（中文）或不加标点（短语式）

### 代码块

- YAML 配置用 `yaml` 语法标注
- Shell 命令用 `bash` 语法标注
- 输出/日志用 `text` 或无标注
- ASCII 图用无标注代码块

### 正反示例标注

当需要对比正确和错误的做法时，使用 ✅/❌ 标注：

```markdown
```yaml
✅ 正确:
allow_origins: "https://app.example.com"
allow_credentials: true

❌ 错误:
allow_origins: "*"
allow_credentials: true
```（去掉此行前的反引号转义）
```

或在表格中使用：

```markdown
| 做法 | 示例 |
|------|------|
| ✅ 推荐 | `allow_origins: "https://app.example.com"` |
| ❌ 避免 | `allow_origins: "*"` |
```

---

## YAML 示例规范

### 完整性要求

**每个 YAML 示例都必须是完整、可用的**。读者应该能直接复制粘贴并 `kubectl apply`（或稍作修改后使用）。

```yaml
# ✅ 完整示例：包含 apiVersion、kind、metadata、spec
apiVersion: gateway.networking.k8s.io/v1
kind: HTTPRoute
metadata:
  name: example-route
  namespace: production
spec:
  parentRefs:
    - name: main-gateway
  rules:
    - matches:
        - path:
            type: PathPrefix
            value: /api/
      backendRefs:
        - name: api-service
          port: 8080
```

**例外**：插件文档的"快速开始"和"配置场景"章节可以只展示 `filters` 片段，但**完整配置示例**章节必须给出包含 HTTPRoute 外壳的完整 YAML。

### 场景排列顺序

从简单到复杂：

1. 最小可用配置（开发环境 / 快速验证）
2. 单一场景（最常见的生产用法）
3. 多选项组合
4. 带安全/认证的配置
5. 高级 / 边缘场景

每个场景都要有标题说明用途。

### 注释规则

- 注释在字段上方，不在行尾
- 中文文档用中文注释，英文文档用英文注释
- 简洁，一行能说清
- 不解释 YAML 语法本身
- 字段名已自解释时不加注释

```yaml
# ✅ 有价值的注释
filters:
  - type: Cors
    config:
      # 允许的前端域名
      allow_origins: "https://app.example.com,https://admin.example.com"
      # 预检请求缓存 24 小时
      max_age: 86400

# ❌ 多余注释
filters:
  - type: Cors          # 类型是 Cors
    config:
      allow_origins: "*" # 设置 allow_origins 为 *
```

### 占位符统一

| 用途 | 占位符 | 不要用 |
|------|--------|--------|
| 域名 | `example.com`、`app.example.com`、`api.example.com` | `mysite.com`、`foo.bar` |
| 管理域名 | `admin.example.com` | `management.xxx` |
| 命名空间 | `production`、`staging`、`edgion-test` | `my-ns`、`test123` |
| Service 名 | `api-service`、`echo-service`、`backend-service` | `svc1`、`my-svc` |
| 端口 | `8080`（HTTP）、`443`（HTTPS）、`80`（HTTP 入口） | 随意数字 |
| Secret 名 | `my-tls-secret`、`<purpose>-tls` | `secret1` |
| Gateway 名 | `main-gateway`、`internal-gateway` | `gw1` |
| Route 名 | `<purpose>-route`，如 `api-route`、`example-route` | `route1` |
| IP 地址 | `192.168.1.x`（内网）、`10.0.0.x`（集群内） | `1.2.3.4` |
| Base64 数据 | `<base64-encoded-cert>` 占位文字 | 真实 base64 长串 |

### 安全警告

涉及安全的示例必须标注环境适用性：

```markdown
⚠️ **仅用于开发！不要在生产环境使用！**
```

### YAML 格式

- 缩进用 2 个空格
- 字符串值如包含特殊字符，使用双引号
- 布尔值用 `true`/`false`，不用 `yes`/`no`
- 空列表用 `[]`，空对象用 `{}`
- 长行不折行（避免 `>` 或 `|` 多行语法，除非确有必要）

---

## 交叉引用规范

### 相对路径规则

所有文档内引用使用**相对路径**，不使用绝对路径或 URL：

- ✅ 相对路径写法（正确）：显示文字加相对路径，如 `[显示文字]` `(../overview.md)` — 路径相对于当前文件
- ✅ 跨目录相对路径（正确）：`[显示文字]` `(../../infrastructure/mtls.md)`
- ❌ 绝对路径（错误）：`/docs/zh-CN/user-guide/http-route/filters/overview.md`

- 链接起点是当前文件所在目录
- 跨语言目录时不要链接（en/ 中不链接到 zh-CN/）
- 链接到目录入口时写 README.md，例如：`[后端配置]` `(../backends/README.md)`（路径需指向存在的文件）

### "相关文档"章节

放在页面末尾，列出 3-6 个最相关的链接：

```text
## 相关文档

- HTTPS 监听器（相对路径，如 ../listeners/https.md）
- EdgionTls 扩展（相对路径，如 ./edgion-tls.md）
- mTLS 配置（相对路径，如 ../../infrastructure/mtls.md）
```

规则：按相关度排序；链接文字是目标文档标题；不写描述。

### 正文中的引用

首次提到其他功能时链接，后续出现不重复链接。章节末尾用"更多 X 见：..."引导到详细文档。

### 外部链接

推荐链接到权威来源（Gateway API 官方、MDN、K8s 官方、RFC）。不链接博客、商业页面、可能失效的文章。

### docs/ 与 skills/ 之间

docs/ 中**不要**链接到 skills/。需要架构信息时链接到 dev-guide/。

### 链接维护

移动/重命名/删除文件时，搜索并更新所有引用。Review 检查清单包含链接有效性检查。
