# 03 测试

> Edgion 测试体系：集成测试（本地 + K8s）、LinkSys 测试、日常调试排错。
> 集成测试架构：Controller (:5800) + Gateway (:5900) + test_server + test_client (suite-based)。

## 文件清单

| 文件 | 主题 | 状态 |
|------|------|------|
| [00-integration-testing.md](00-integration-testing.md) | 集成测试运行、新增、调试全流程（核心测试文档） | ✅ 完整 |
| [01-k8s-integration-testing.md](01-k8s-integration-testing.md) | K8s 环境测试差异、改造清单、执行阶段 | ✅ 完整 |
| [02-link-sys-testing.md](02-link-sys-testing.md) | LinkSys 集成测试（bash + Docker Compose + Admin API） | ✅ 完整 |
| [03-debugging.md](03-debugging.md) | 日常开发调试、Admin API、edgion-ctl、问题速查 | TODO |

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
- `examples/test/code/` — 测试代码（client/server/validator）
- `examples/test/conf/` — 测试配置（按资源类型组织）
- `examples/test/scripts/` — 测试脚本
- `integration_testing/` — 运行时工作目录（gitignored）
