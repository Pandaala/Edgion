---
name: batch-conformance-debugging
description: 批量 conformance 测试失败的日志采集、分析和诊断方法，用于定位测试间状态泄漏和并发问题
---

# 批量 Conformance 测试调试

当测试单独跑通过但批量跑失败时，使用本指南。核心思路：**采集三路日志 → 找关键模式 → 时序关联 → 定位根因**。

## 1. 日志采集

批量测试前启动三路日志采集，所有日志带时间戳：

```bash
# 终端 1: Controller 日志
kubectl logs -f deployment/edgion-controller -n edgion-system --timestamps 2>&1 | tee /tmp/controller-batch.log

# 终端 2: Gateway 日志
kubectl logs -f deployment/edgion-gateway -n edgion-system --timestamps 2>&1 | tee /tmp/gateway-batch.log

# 终端 3: 测试输出
./scripts/orbstack-dev.sh test --core 2>&1 | tee /tmp/test-batch.log
```

单独跑失败的测试做对比：

```bash
kubectl logs -f deployment/edgion-controller -n edgion-system --timestamps 2>&1 | tee /tmp/controller-single.log &
kubectl logs -f deployment/edgion-gateway -n edgion-system --timestamps 2>&1 | tee /tmp/gateway-single.log &
./scripts/orbstack-dev.sh test --run-test <FailingTestName> 2>&1 | tee /tmp/test-single.log
```

## 2. 关键日志模式

### Controller 端

| 模式 | 含义 | 搜索方式 |
|------|------|---------|
| queue depth 告警 | requeue 队列积压（达 80% 容量时告警） | `grep -i "queue.*depth\|queue.*warn\|capacity"` |
| requeue 风暴 | 同一个 key 被反复 requeue | `grep "enqueue\|requeue" \| sort \| uniq -c \| sort -rn` |
| status patch 冲突 | ResourceVersion 冲突导致 status 写入失败 | `grep -i "conflict\|resource.version\|status.*fail\|status.*error"` |
| 删除/创建时序重叠 | 资源还没删完就被重新创建 | `grep "on_delete\|on_change\|on_add"` 看同一个 key 的事件间隔 |
| trigger chain 深度 | 级联 requeue 过深被截断 | `grep -i "trigger.*cycle\|trigger.*depth\|cascade"` |
| 处理耗时 | 单个 work item 处理时间过长 | `grep -i "process.*ms\|elapsed\|slow"` |

### Gateway 端

| 模式 | 含义 | 搜索方式 |
|------|------|---------|
| `TOO_OLD_VERSION` | 事件缓冲区溢出，触发全量 re-list | `grep "TOO_OLD_VERSION\|EVENTS_LOST"` |
| `SERVER_RELOAD` | Controller 重启导致全量重同步 | `grep "SERVER_RELOAD\|SERVER_ID_MISMATCH\|server_id"` |
| re-list 触发 | Gateway 发起全量列表请求（非启动阶段） | `grep -i "re-list\|full.*list\|list.*request"` |
| sync version 跳跃 | 版本号非连续递增 | `grep "sync_version\|from_version"` |
| 路由表更新 | 路由添加/删除时间点 | `grep -i "route.*add\|route.*delete\|route.*update\|partial_update"` |
| backoff 触发 | 连接断开后重连等待 | `grep -i "backoff\|reconnect\|retry"` |

### Test Output

| 模式 | 含义 | 搜索方式 |
|------|------|---------|
| 意外 200 | 旧路由还在，请求被错误转发 | `grep "expected 404.*got 200\|unexpected.*200"` |
| 意外 404 | 新路由还没生效 | `grep "expected 200.*got 404\|unexpected.*404"` |
| status condition 不匹配 | status 来自上一个测试 | `grep "condition.*expected\|status.*mismatch\|Accepted.*False"` |
| 超时 | 等待条件超时 | `grep -i "timeout\|timed out\|deadline"` |

## 3. 时序关联分析

**目标：** 把测试失败的时间点和 controller/gateway 日志对齐，判断属于哪种失败模式。

### 步骤

1. **从 test output 找失败时间点**
   ```bash
   grep -E "FAIL|--- FAIL" /tmp/test-batch.log
   ```

2. **在 controller 日志中找该时间窗口（前后 30 秒）的事件**
   ```bash
   # 假设失败时间是 14:32:15
   awk '/14:31:45/,/14:32:45/' /tmp/controller-batch.log
   ```

3. **在 gateway 日志中找同一时间窗口的 sync 事件**
   ```bash
   awk '/14:31:45/,/14:32:45/' /tmp/gateway-batch.log
   ```

4. **判断失败模式**

| 观察到的现象 | 失败模式 | 根因方向 |
|-------------|---------|---------|
| 测试 N 的 Route 删除事件在测试 N+1 开始后才出现 | 旧路由残留 | Controller 处理延迟或 Gateway sync 延迟 |
| 测试 N+1 的 Route 创建事件已处理但 Gateway 没收到 | 新路由没到 | gRPC sync 延迟或事件丢失 |
| Status patch 在测试 N+1 的断言之后才写入 | Status 滞后 | Status 写入队列积压或冲突重试 |
| 同一个 key 反复 enqueue/dequeue | Requeue 风暴 | 级联 requeue 放大 |

## 4. 单跑 vs 批量对比

对同一个失败测试：

```bash
# 对比 controller 日志中该测试资源的处理时序
diff <(grep "same-namespace\|httproute" /tmp/controller-single.log) \
     <(grep "same-namespace\|httproute" /tmp/controller-batch.log)
```

重点关注：
- **事件间隔差异**：批量跑时删除到重建的间隔是否更短
- **队列深度差异**：批量跑时是否出现了单跑没有的积压
- **额外事件**：批量跑时是否有其他测试的资源事件穿插进来

## 5. 诊断结论模板

分析完成后，用以下格式记录：

```
测试: <TestName>
失败模式: 旧路由残留 / 新路由没到 / Status 滞后 / Requeue 风暴
证据:
  - [时间戳] controller: <关键日志行>
  - [时间戳] gateway: <关键日志行>
  - [时间戳] test: <失败断言>
时序问题: 测试 N 的 <事件> 和测试 N+1 的 <事件> 间隔仅 <X>ms
建议修复方向: <具体建议>
```

## 相关文件

- 批量修复计划: `tasks/todo/conformance-fix-batches/README.md`
- Batch 7 详情: `tasks/todo/conformance-fix-batches/batch-07-batch-concurrency.md`
- Controller workqueue: `src/core/controller/conf_mgr/sync_runtime/workqueue.rs`
- Gateway sync: `src/core/gateway/conf_sync/cache_client/event_dispatch.rs`
- Conformance 测试入口: `examples/gateway-api-conformance/conformance_test.go`
