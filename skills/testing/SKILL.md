---
name: testing
description: Testing skill for Edgion. Use when running, debugging, or extending integration tests, K8s tests, LinkSys tests, or configuration sync validation workflows.
---

# 03 测试

> Edgion 测试体系：集成测试（本地 + K8s）、LinkSys 测试、日常调试排错。
> 集成测试架构：Controller (:5800) + Gateway (:5900) + test_server + test_client (suite-based)。

## 文件清单

| 文件 | 主题 | 状态 |
|------|------|------|
| [00-integration-testing.md](00-integration-testing.md) | 集成测试运行、新增、调试全流程（核心 workflow） | ✅ 已重构 |
| [01-k8s-integration-testing.md](01-k8s-integration-testing.md) | K8s 环境测试差异、改造清单、执行阶段 | ✅ 完整 |
| [02-link-sys-testing.md](02-link-sys-testing.md) | LinkSys 集成测试（bash + Docker Compose + Admin API） | ✅ 完整 |
| [03-debugging.md](03-debugging.md) | 日常开发调试、Admin API、edgion-ctl、Access Log Store、Metrics、问题速查 | ✅ 已重构 |
| [04-conf-sync-leak-testing.md](04-conf-sync-leak-testing.md) | 配置同步泄漏检测（基础循环 + 高级边界场景） | ✅ 完整 |

## 快速入门

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
