# CLAUDE.md — Edgion 项目指令

## 项目概要

Edgion 是一个基于 Rust + Pingora 的 Kubernetes 网关，采用 Controller–Gateway 分离架构，gRPC 配置同步。
支持 HTTP/1.1、HTTP/2、gRPC、TCP、UDP、TLS、WebSocket，含插件系统、负载均衡、TLS/mTLS。

**四个二进制**：Controller（K8s 资源处理）、Gateway（数据面代理）、edgion-ctl（CLI 调试工具）、Center（联邦中心）。

**Feature Flags**（互斥，构建时注意）：
- TLS 后端：`boringssl`（默认）| `openssl` | `rustls`
- 内存分配器：`allocator-jemalloc`（默认）| `allocator-mimalloc` | `allocator-system`

## 知识体系

项目知识分三层，**按需渐进加载**，不要一次全部读取：

1. **`AGENTS.md`** — 跨平台 Agent 指令入口，含常见工作流和命令速查
2. **`skills/SKILL.md`** — 面向任务的知识库总索引，所有深入的架构、功能、编码知识从这里导航
3. **`docs/`** — 面向人类的用户文档（非 Agent 起点）

### 何时加载 Skills

- **任何需要项目上下文的任务**：先读 `skills/SKILL.md` 的快速定位表，按关键词找到最短路径
- **理解架构**（内部实现）→ `skills/01-architecture/`
- **查功能/配置 Schema**（怎么用/怎么配）→ `skills/02-features/`
- **写代码**（编码规范）→ `skills/03-coding/`
- **测试验证** → `skills/05-testing/`
- **资源相关任务**：`01-architecture/05-resources/`（实现）+ `02-features/03-resources/`（Schema），通常两边都要看

### Skills 导航规则

1. 渐进式加载：`skills/SKILL.md` → 分类 SKILL.md → 具体文件。只加载当前任务需要的最小子树
2. 快速定位优先：`skills/SKILL.md` 有快速定位表，按关键词直接给出最短路径
3. 资源相关任务：先到 `01-architecture/05-resources/SKILL.md` 找架构文档，再到 `02-features/` 查 Schema

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

## 常见工作流速查

| 场景 | 起点 |
|------|------|
| 新功能（需架构上下文） | `skills/SKILL.md` → `01-architecture/` → `02-features/` → `05-testing/` |
| 添加新资源类型 | `skills/01-architecture/01-controller/09-add-new-resource/00-guide.md` |
| 添加/调试 HTTP 插件 | `skills/01-architecture/02-gateway/12-edgion-plugin-dev.md` |
| 添加/调试 Stream 插件 | `skills/01-architecture/02-gateway/13-stream-plugin-dev.md` |
| 调试路由/TLS/同步问题 | `skills/05-testing/SKILL.md` → 相关架构文档 |
| 配置 Schema 参考 | `skills/02-features/02-config/SKILL.md` |
| CI/发布自动化 | `skills/09-misc/02-github-workflow.md` |
| 任务管理与模板 | `skills/07-tasks/SKILL.md` |
