---
name: 04-testing
description: Testing skill for Edgion. Use when writing, running, or extending unit tests, integration tests, K8s tests, LinkSys tests, or configuration sync validation workflows.
---

# 04 测试

> Edgion 测试体系，目标：单元测试 + 集成测试达到 **99% 代码覆盖率**。
> 分为**标准测试**（覆盖率保证）和**专项测试**（特定场景验证）两部分。

## 文件清单

### Part 1 — 标准测试（覆盖率保证）

| 文件 | 主题 | 状态 |
|------|------|------|
| [00-unit-testing.md](00-unit-testing.md) | 单元测试：inline test module、运行方式、编写规范、与集成测试的分工 | ✅ 完整 |
| [01-integration-testing.md](01-integration-testing.md) | 集成测试：架构、运行流程、新增测试步骤 | ✅ 已重构 |
| [02-k8s-integration-testing.md](02-k8s-integration-testing.md) | K8s 集成测试：与本地测试差异、改造清单、执行阶段 | ✅ 完整 |

### Part 2 — 专项测试（特定场景验证）

| 文件 | 主题 | 状态 |
|------|------|------|
| [03-link-sys-testing.md](03-link-sys-testing.md) | LinkSys 集成测试：bash 测试流程、Admin API 验证、Docker Compose | ✅ 完整 |
| [04-conf-sync-leak-testing.md](04-conf-sync-leak-testing.md) | 配置同步泄漏检测：基础循环测试 + 高级边界场景 | ✅ 完整 |

## 快速入门

### 运行单元测试
```bash
cargo test --all
```

### 运行集成测试
```bash
./examples/test/scripts/integration/run_integration.sh
```

### 查看测试报告
```bash
cat ${WORK_DIR}/report.log
```

### 关键测试目录
- `examples/code/` — 测试代码（client/server/validator）
- `examples/test/conf/` — 测试配置（按资源类型组织）
- `examples/test/scripts/` — 测试脚本
- `integration_testing/` — 运行时工作目录（gitignored）

### 需要时再读
- [references/integration-suite-map.md](references/integration-suite-map.md) — suite family、conf 目录、Rust suite、`--gateway` 映射
- [references/test-server-capabilities.md](references/test-server-capabilities.md) — `test_server` 端口、HTTP 端点、OIDC/auth、mirror、TLS 后端能力

### 调试排障（已移至独立目录）
- [../05-debugging/00-debugging.md](../05-debugging/00-debugging.md) — 日常开发调试、Admin API、edgion-ctl、问题速查
