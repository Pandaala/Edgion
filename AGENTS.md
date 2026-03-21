# Edgion Agent 指南

本文件是跨平台、仓库级别的编码 Agent 指令入口。
将其作为 Codex、Cursor、Claude 及其他 Agent 工具的规范共享层。

## 从这里开始

当任务需要项目上下文时，从 `skills/SKILL.md` 开始。

### Skills 导航规则

1. **渐进式加载**：`skills/SKILL.md` → 分类 SKILL.md → 具体文件。只加载当前任务需要的最小子树，不要一次全部加载。
2. **快速定位优先**：`skills/SKILL.md` 有"快速定位"表，按关键词（资源名、子系统名、场景）直接给出最短路径，优先使用它而不是逐级浏览。
3. **三层定位**：理解架构 → `01-architecture/`；查功能/配置 Schema → `02-features/`；写代码 → `01-architecture/`（开发指南）+ `03-coding/`。
4. **资源相关任务**：先到 `01-architecture/05-resources/SKILL.md` 找到该资源的架构文档，再到 `02-features/` 查功能 Schema。
5. **`docs/` 不是起点**：`docs/` 面向人类用户，`skills/` 面向 AI 和开发者。任务上下文从 skills 获取。

### 任务生命周期

任务模板、生命周期阶段、阶段→Skills 映射、裁剪规则统一见 `skills/07-task/SKILL.md`。

## 常见工作流

- 需要架构上下文的新功能：
  1. 阅读 `skills/SKILL.md`
  2. 阅读 `skills/01-architecture/SKILL.md`
  3. 仅加载直接相关的架构文件
  4. 然后阅读 `skills/02-features/SKILL.md`（功能/配置 Schema 参考）
  5. 最后使用 `skills/05-testing/SKILL.md` 进行验证

- 添加新资源类型：
  1. `skills/01-architecture/01-controller/09-add-new-resource/00-guide.md`
  2. 从该工作流中选择最接近的模式参考（`route-like`、`controller-only`、`plugin-like`、`cluster-scoped`）
  3. `skills/01-architecture/00-common/03-resource-system.md`
  4. `skills/01-architecture/01-controller/03-config-center/SKILL.md`
  5. `skills/05-testing/00-integration-testing.md`

- 调试路由、TLS 或同步问题：
  1. `skills/05-testing/SKILL.md`
  2. 当症状出现在 Controller 重启/重载之后，或 Gateway 日志出现 `Unknown kind` 时：`skills/01-architecture/01-controller/03-config-center/02-kubernetes/00-lifecycle.md`
  3. 关于 Controller↔Gateway gRPC 同步行为：`skills/01-architecture/03-controller-gateway-link/SKILL.md`
  4. 当涉及 Gateway API 语义时：`skills/08-gateway-api/SKILL.md`
  5. TLS 网关路由问题：`skills/09-misc/debugging-tls-gateway.md`

- 了解 Controller/Gateway 配置和路径行为：
  1. `skills/02-features/02-config/SKILL.md`
  2. 加载 Controller、Gateway 或 `EdgionGatewayConfig` 对应的配置 Schema 文件
  3. 当相对路径行为相关时：`docs/zh-CN/dev-guide/work-directory.md`

- 在修改清单或文档之前了解 `edgion.io/*` 键：
  1. `skills/02-features/10-annotations/00-annotations-overview.md`
  2. 加载 `metadata.annotations`、`options` 或保留/仅测试键的对应参考
  3. 更新过时的示例，而不是向前复制遗留键

- 添加或调试 HTTP 插件行为：
  1. `skills/01-architecture/02-gateway/12-edgion-plugin-dev.md`
  2. `skills/03-coding/observability/00-access-log.md`
  3. `skills/05-testing/00-integration-testing.md`

- 添加或调试 Stream 插件行为：
  1. `skills/01-architecture/02-gateway/13-stream-plugin-dev.md`
  2. `skills/02-features/10-annotations/00-annotations-overview.md`
  3. `skills/05-testing/00-integration-testing.md`

- 修改 CI 或发布自动化：
  1. `skills/09-misc/02-github-workflow.md`
  2. `skills/09-misc/00-local-build.md`
  3. `skills/09-misc/01-docker-build.md`

## 常用命令

```bash
# 构建
cargo build
cargo build --bin edgion-controller
cargo build --bin edgion-gateway

# 检查
cargo check --all-targets
cargo fmt --all -- --check
cargo clippy --all-targets
cargo test --all
make check-agent-docs

# 定向集成测试
./examples/test/scripts/integration/run_integration.sh --no-prepare -r <Resource> -i <Item>

# 完整集成测试
./examples/test/scripts/integration/run_integration.sh
```

## 知识源规则

- 保持 `AGENTS.md` 作为规范的跨平台 Agent 入口。
- 保持 `skills/` 作为规范的面向任务的知识层。
- 保持 `docs/` 作为规范的面向人类的文档层。
- 不要在 `skills/` 和 `docs/` 中重复相同的详细内容；优先选择一个规范来源并链接到它。
- 如果工具需要供应商特定的封装（如 `CLAUDE.md` 或 `.cursor/rules/`），保持该封装精简并指回本文件。

## 人类提示指南

如果工具会读取仓库指令，人类不需要在聊天中粘贴大量文档列表。
好的提示应该简短且面向任务，例如：

- "遵循 `AGENTS.md`。此功能在实现前需要架构上下文。"
- "使用仓库技能来理解资源管道，然后添加新资源。"
- "使用测试技能和 Gateway API 说明来调试此集成回归。"

更详细的协作模式请参见：

- `docs/zh-CN/dev-guide/ai-agent-collaboration.md`
- `docs/en/dev-guide/ai-agent-collaboration.md`
- `docs/zh-CN/dev-guide/knowledge-source-map.md`
- `docs/en/dev-guide/knowledge-source-map.md`
