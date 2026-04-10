# 子任务

## 总进度

已完成 0/8

## 执行顺序建议

0 → 1 → 6 → 4 → 2 → 3 → 5 → 7（先基础设施，再小范围微调，再批量插件页，最后验收）

---

## 模块 0：基础设施

> DIRECTORY.md 同步 + 缺失 README.md 补全。其他模块依赖此模块。

- **状态**: pending

### M0-1: 更新 docs/DIRECTORY.md

- [ ] 补 `zh-CN/user-guide/http-route/backends/circuit-breaker.md` 条目
- [ ] 补 `superpowers/` 目录树（或决定是否从 DIRECTORY.md 中排除）
- [ ] 确认 zh-CN 独有文件（promotion/、openid-connect-review-card.md）在目录索引中有标注

### M0-2: 补缺失的 README.md（en + zh-CN 各 6 个）

以下目录有多个文件但缺少导航 README.md：

| 目录 | en | zh-CN |
|------|-----|-------|
| `user-guide/http-route/` | ❌ | ❌ |
| `user-guide/grpc-route/` | ❌ | ❌ |
| `user-guide/tcp-route/` | ❌ | ❌ |
| `user-guide/udp-route/` | ❌ | ❌ |
| `user-guide/edgion-plugins/` | ❌ | ❌ |
| `ops-guide/gateway/` | ❌ | ❌ |

每个 README.md 按 `skills/11-doc/00-doc-structure.md` 的规范：一级标题 + 一句话说明 + 文档导航表格。

---

## 模块 1：多语言对称

> 补齐 en/ 缺失文件，处理 zh-CN 独有文件。

- **状态**: pending

### M1-1: en/ 补 circuit-breaker.md

- [ ] 翻译 `docs/zh-CN/user-guide/http-route/backends/circuit-breaker.md` 到 `docs/en/user-guide/http-route/backends/circuit-breaker.md`
- [ ] 更新 en/ 侧 `backends/README.md` 导航

### M1-2: 处理 openid-connect-review-card.md

- [ ] 决策：此文件是内部审查卡片还是用户文档？
  - 若为用户文档 → 翻译到 en/
  - 若为内部文档 → 移出 docs/ 或移到 docs/review/
- [ ] 执行决策

---

## 模块 2：HTTP Route 插件页对齐

> `docs/*/user-guide/http-route/filters/edgion-plugins/` 下 18 篇。大部分已较好，需补缺。

- **状态**: pending

### M2-1: rate-limit.md 重构（问题最多的一篇）

- [ ] 补 🔌 Edgion 扩展标注（前 10 行）
- [ ] 重组章节顺序：快速开始 → 配置参数 → 常见配置场景 → FAQ → 相关链接
- [ ] 参数表从五列改为标准四列（参数/类型/默认值/说明）
- [ ] 补完整 YAML 示例（含 HTTPRoute 外壳）
- [ ] 补 `## 相关链接` 章节

### M2-2: request-restriction.md 补标注

- [ ] 补 🔌 Edgion 扩展标注

### M2-3: 补「相关文档/链接」章节（8 篇缺失）

| 文件 | 状态 |
|------|------|
| basic-auth.md | ❌ |
| csrf.md | ❌ |
| ip-restriction.md | ❌ |
| jwt-auth.md | ❌ |
| proxy-rewrite.md | ❌ |
| response-rewrite.md | ❌ |
| rate-limit.md | ❌（M2-1 覆盖） |
| request-restriction.md | ❌ |

### M2-4: en/ 侧同步

- [ ] 以上所有修改同步到 `docs/en/user-guide/http-route/filters/edgion-plugins/`

---

## 模块 3：独立插件页对齐

> `docs/*/user-guide/edgion-plugins/` 下 9 篇。问题最多，几乎全不达标。

- **状态**: pending

### M3-1: 全部 9 篇补 🔌 Edgion 扩展标注

| 文件 | 状态 |
|------|------|
| ctx-setter.md | ❌ |
| forward-auth.md | ❌ |
| jwe-decrypt.md | ❌ |
| ldap-auth.md | ❌ |
| openid-connect.md | ❌ |
| rate-limit.md | ❌ |
| real-ip.md | ❌ |
| region-route.md | ❌ |
| openid-connect-review-card.md | ❌（取决于 M1-2 决策） |

### M3-2: 全部 9 篇补「相关文档」章节

同上文件列表，每篇末尾添加 `## 相关文档` + 3-6 个相关链接。

### M3-3: 补配置参数表

- [ ] openid-connect.md：补标准四列参数表
- [ ] region-route.md：补标准四列参数表

### M3-4: 章节顺序对齐

检查所有 9 篇，按插件模板重排：快速开始 → 配置参数 → 常见配置场景 → FAQ（推荐） → 相关文档。

### M3-5: en/ 侧同步

- [ ] 以上所有修改同步到 `docs/en/user-guide/edgion-plugins/`

---

## 模块 4：Route 总览页对齐

> 4 种 Route 类型的 overview.md。

- **状态**: pending

### M4-1: http-route/overview.md（差距最大）

- [ ] 补「完整示例」独立章节（覆盖 matches + filters + backends 的较长 YAML）
- [ ] 补「子文档导航」表格（matches/、filters/、backends/、resilience/）
- [ ] 拆分核心概念为 ParentRefs / Rules / Matches / Filters·Plugins / BackendRefs 子节

### M4-2: 其余 3 种 Route

对以下文件按 Route 总览模板检查并补齐：
- [ ] grpc-route/overview.md
- [ ] tcp-route/overview.md
- [ ] udp-route/overview.md

### M4-3: en/ 侧同步

---

## 模块 5：运维文档对齐

> `docs/*/ops-guide/` 下约 20 篇。

- **状态**: pending

### M5-1: tls-termination.md 修复

- [ ] Gateway YAML 示例补 `namespace` 字段
- [ ] 「创建 Secret」部分拆分为独立的 `bash` 和 `yaml` 代码块

### M5-2: 补「相关文档」章节

以下页面缺少 `## 相关文档`：
- [ ] edgion-cli.md
- [ ] observability/metrics.md
- [ ] observability/access-log.md
- [ ] gateway/gateway-class.md
- [ ] gateway/overview.md
- [ ] gateway/http-to-https-redirect.md
- [ ] gateway/tls/acme.md

### M5-3: en/ 侧同步

---

## 模块 6：Getting Started 微调

> 影响面最小，改动最少。

- **状态**: pending

### M6-1: first-gateway.md Step 1 补收尾

- [ ] Step 1（GatewayClass）末尾补"这一步确定了什么"总结段落（Step 2、3 已有此模式）

### M6-2: YAML namespace 完整性

- [ ] 检查 getting-started/ 下所有 YAML 示例，确认有 `namespace` 字段或有说明
- [ ] 同步检查 core-concepts.md、installation-and-deployment.md

### M6-3: en/ 侧同步

---

## 模块 7：最终验证

> 所有修改完成后的收尾检查。

- **状态**: pending

### M7-1: 全量链接有效性检查

- [ ] 扫描所有 docs/ 下 .md 文件中的 `[text](path)` 链接
- [ ] 确认目标文件存在，无悬挂链接

### M7-2: en/zh-CN 文件对称性确认

- [ ] 列出 en/ 和 zh-CN/ 的所有文件路径，diff 对比
- [ ] 确认仅 promotion/ 为 zh-CN 独有（预期例外）

### M7-3: DIRECTORY.md 最终对账

- [ ] 用脚本对比 DIRECTORY.md 列出的文件与实际文件系统
- [ ] 确认完全一致
