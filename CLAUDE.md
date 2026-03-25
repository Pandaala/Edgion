# Edgion — AI Agent 项目指令

## 项目概要

Edgion 是一个基于 Rust + Pingora 的 Kubernetes 网关，采用 Controller–Gateway 分离架构，gRPC 配置同步。
支持 HTTP/1.1、HTTP/2、gRPC、TCP、UDP、TLS、WebSocket，含插件系统、负载均衡、TLS/mTLS。

**四个二进制**：Controller（K8s 资源处理）、Gateway（数据面代理）、edgion-ctl（CLI 调试工具）、Center（联邦中心）。

**Feature Flags**（互斥，构建时注意）：
- TLS 后端：`boringssl`（默认）| `openssl` | `rustls`
- 内存分配器：`allocator-jemalloc`（默认）| `allocator-mimalloc` | `allocator-system`

## 知识体系

当任务需要项目上下文时，从 `skills/SKILL.md` 开始，**按需渐进加载**，不要一次全部读取。

### Skills 导航规则

1. **渐进式加载**：`skills/SKILL.md` → 分类 SKILL.md → 具体文件。只加载当前任务需要的最小子树。
2. **快速定位优先**：`skills/SKILL.md` 有快速定位表，按关键词直接给出最短路径，优先使用它而不是逐级浏览。
3. **三层定位**：理解架构 → `01-architecture/`；查功能/配置 Schema → `02-features/`；写代码 → `01-architecture/`（开发指南）+ `03-coding/`。
4. **资源相关任务**：先到 `01-architecture/05-resources/SKILL.md` 找架构文档，再到 `02-features/` 查功能 Schema。通常两边都要看。
5. **`docs/` 不是起点**：`docs/` 面向人类用户，`skills/` 面向 AI 和开发者。

### 任务生命周期

任务模板、生命周期阶段、阶段→Skills 映射、裁剪规则统一见 `skills/07-tasks/SKILL.md`。

## 常见工作流

- **新功能（需架构上下文）**：
  1. `skills/SKILL.md` → `skills/01-architecture/SKILL.md` → 仅加载直接相关的架构文件
  2. `skills/02-features/SKILL.md`（功能/配置 Schema 参考）
  3. `skills/05-testing/SKILL.md`（验证）

- **添加新资源类型**：
  1. `skills/01-architecture/01-controller/09-add-new-resource/00-guide.md`
  2. 选择最接近的模式参考（`route-like`、`controller-only`、`plugin-like`、`cluster-scoped`）
  3. `skills/01-architecture/00-common/03-resource-system.md`
  4. `skills/01-architecture/01-controller/03-config-center/SKILL.md`
  5. `skills/05-testing/00-integration-testing.md`

- **添加/调试 HTTP 插件**：
  1. `skills/01-architecture/02-gateway/12-edgion-plugin-dev.md`
  2. `skills/03-coding/observability/00-access-log.md`
  3. `skills/05-testing/00-integration-testing.md`

- **添加/调试 Stream 插件**：
  1. `skills/01-architecture/02-gateway/13-stream-plugin-dev.md`
  2. `skills/02-features/05-annotations/00-annotations-overview.md`
  3. `skills/05-testing/00-integration-testing.md`

- **调试路由/TLS/同步问题**：
  1. `skills/05-testing/SKILL.md`
  2. Controller 重启/Gateway 日志 `Unknown kind` → `skills/01-architecture/01-controller/03-config-center/02-kubernetes/00-lifecycle.md`
  3. Controller↔Gateway gRPC 同步 → `skills/01-architecture/03-controller-gateway-link/SKILL.md`
  4. Gateway API 语义 → `skills/08-gateway-api/SKILL.md`
  5. TLS 网关路由 → `skills/09-misc/debugging-tls-gateway.md`

- **了解配置和路径行为**：
  1. `skills/02-features/02-config/SKILL.md`
  2. 加载 Controller、Gateway 或 `EdgionGatewayConfig` 对应的配置 Schema 文件
  3. 相对路径行为 → `docs/zh-CN/dev-guide/work-directory.md`

- **修改 `edgion.io/*` 注解**：
  1. `skills/02-features/05-annotations/00-annotations-overview.md`
  2. 加载对应的 `metadata.annotations`、`options` 或保留/仅测试键参考
  3. 更新过时示例，不要向前复制遗留键

- **CI/发布自动化**：
  1. `skills/09-misc/02-github-workflow.md`
  2. `skills/09-misc/00-local-build.md`
  3. `skills/09-misc/01-docker-build.md`

## 常用命令

```bash
# 全量检查
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

## 编码约束

- 语言：Rust，遵循项目现有风格
- 提交前确保 `cargo check`、`cargo fmt --check`、`cargo clippy` 通过
- 日志 ID、可观测性规范见 `skills/03-coding/`
- 不要引入安全漏洞（注入、XSS 等 OWASP Top 10）
- 改动最小化，不做未被要求的重构或功能添加

## 知识源规则

- `skills/` 是面向任务的知识层（AI 和开发者）
- `docs/` 是面向人类的文档层
- 不要在两者间重复详细内容，选择一个规范来源并链接
