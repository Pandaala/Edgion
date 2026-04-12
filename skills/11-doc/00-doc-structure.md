# 文档目录结构规范

## 顶层结构

```
docs/
├── README.md              # 项目级 README（语言入口）
├── DIRECTORY.md           # 完整目录树索引（AI 和人类共用）
├── en/                    # English documentation
├── zh-CN/                 # 中文文档
├── ja/                    # 日本語ドキュメント（预留）
├── refactoring/           # 内部重构设计文档（不面向用户）
├── review/                # 代码审查记录（不面向用户）
└── superpowers/           # 设计方案和计划（不面向用户）
```

`en/` 和 `zh-CN/` 内部结构必须完全对称。

## 四层体系

每种语言目录下有四层，面向不同受众和阶段：

| 层 | 目录 | 受众 | 定位 |
|----|------|------|------|
| 1 | `getting-started/` | 新用户 | 从零到第一次成功运行。5 分钟能跑通 |
| 2 | `user-guide/` | 日常使用者 | 按功能组织的使用指南。Route 类型、插件、匹配规则、后端配置 |
| 3 | `ops-guide/` | 运维人员 | 网关运维：Gateway 配置、TLS、监听器、可观测性、基础设施 |
| 4 | `dev-guide/` | 开发者/贡献者 | 架构概述、插件开发、资源注册、日志系统、CI/CD |

### 层的边界

- **getting-started** 只解释"跑通需要知道的最少知识"，不展开所有选项
- **user-guide** 按功能聚合，每个 Route 类型独立子目录
- **ops-guide** 关注"部署和运维"，不涉及业务路由规则
- **dev-guide** 面向向 Edgion 贡献代码的人，不面向仅使用 Edgion 的用户

## 目录命名规则

| 规则 | 示例 | 反例 |
|------|------|------|
| kebab-case | `http-route/`, `backend-tls.md` | `httpRoute/`, `backend_tls.md` |
| 目录名用复数名词或功能名 | `filters/`, `backends/`, `matches/` | `filter/` |
| 文件名用功能名，不加序号前缀 | `cors.md`, `session-persistence.md` | `01-cors.md` |
| README.md 作为目录索引 | `filters/README.md` | `filters/index.md` |

**例外**：getting-started 可能需要暗示阅读顺序，但仍用功能名而非序号。

## README.md 规范

每个子目录的 README.md 充当该目录的导航页：

```markdown
# <目录主题>

<一句话说明这个目录覆盖什么>

## 内容

| 文档 | 说明 |
|------|------|
| xxx.md | 一句话描述 |
| yyy.md | 一句话描述 |
```

要求：
- 一级标题是目录主题，不是"目录"或"Index"
- 表格列出该目录下所有文档，每个一句话描述
- 如果有子目录，也列出并标注"目录"

## DIRECTORY.md 同步规则

`docs/DIRECTORY.md` 是整棵文档树的完整索引。

**触发更新**：
- 新增文档文件
- 删除文档文件
- 移动或重命名文档文件
- 新增或删除子目录

**不触发更新**：
- 仅修改文档内容（文件路径未变）

**更新方式**：手动维护。修改文档结构的 PR 必须同步更新 DIRECTORY.md。

## 新增文档放置决策树

```
你要写的文档是关于…

├── 用户第一次接触、快速上手？
│   └── getting-started/
│
├── 某种路由（HTTP/gRPC/TCP/UDP）的使用方式？
│   └── user-guide/<route-type>/
│   ├── 匹配规则 → matches/
│   ├── 过滤器/插件 → filters/
│   ├── 后端配置 → backends/
│   └── 韧性（超时/重试/会话保持） → resilience/
│
├── 独立插件（不绑定特定 Route 类型）？
│   └── user-guide/edgion-plugins/
│
├── Gateway/Listener/TLS 配置？
│   └── ops-guide/gateway/
│
├── 可观测性（日志、指标）？
│   └── ops-guide/observability/
│
├── 基础设施（Secret、ReferenceGrant、mTLS）？
│   └── ops-guide/infrastructure/
│
├── 开发/贡献指南（架构、插件开发、CI）？
│   └── dev-guide/
│
└── 推广材料（白皮书、介绍文）？
    └── promotion/（仅 zh-CN）
```

## 与 skills/ 的关系

- `docs/` 是面向人类的文档层
- `skills/` 是面向 AI 和开发者的知识层
- 不要在两者间重复详细内容
- docs/ 中如需涉及架构细节，链接到 dev-guide/ 的对应文档，不要内联 skills/ 的内容
