---
name: 05-debugging
description: Debugging and troubleshooting skill for Edgion. Use when local code changes break routing, plugins, sync, readiness, or runtime behavior.
---

# 05 调试与排障

> 面向日常开发排障：代码改完后快速定位问题层次。
> 工具链：Admin API、edgion-ctl 三视图、Access Log Store、Metrics、resource_diff。

## 文件清单

| 文件 | 主题 | 状态 |
|------|------|------|
| [00-debugging.md](00-debugging.md) | 日常开发调试、Admin API、edgion-ctl、Access Log Store、Metrics、问题速查 | ✅ 已重构 |

## 快速入口

### 最快的本地调试方式

```bash
./examples/test/scripts/integration/run_integration.sh --keep-alive -r <Resource> -i <Item>
```

### 三视图定位问题层次

```bash
./target/debug/edgion-ctl get httproute -n default
./target/debug/edgion-ctl -t server get httproute -n default
./target/debug/edgion-ctl -t client get httproute -n default
```

### 相关知识

- TLS 专项排障：[../10-misc/debugging-tls-gateway.md](../10-misc/debugging-tls-gateway.md)
- 集成测试运行：[../04-testing/01-integration-testing.md](../04-testing/01-integration-testing.md)
