# SSL Logger 无界 Channel 内存风险审查

> 审查日期: 2026-03-30
> 严重程度: **中高** (生产环境可能导致 OOM)
> 文件: `src/core/gateway/observe/logs/ssl_log.rs`

---

## 1. 问题描述

`SslLogger` 在 TLS 回调与异步日志写入之间使用 `tokio::sync::mpsc::unbounded_channel()` 作为桥接。该设计确保了 `log_ssl()` 在 TLS 握手回调中绝不阻塞——这是正确的安全约束，因为 TLS 回调运行在 OpenSSL/BoringSSL 的同步上下文中，阻塞会直接影响所有 TLS 连接。

然而，**无界 channel 意味着内存使用没有上限**。当下游 `LocalFileWriter` 的消费速度跟不上 TLS 连接的产生速度时，消息会在 channel 内无限堆积，最终可能耗尽进程内存（OOM）。

## 2. 受影响的代码

### 主要文件

**`src/core/gateway/observe/logs/ssl_log.rs`** — 第 23、29 行:

```rust
pub struct SslLogger {
    // 无界 channel — 无内存上限
    tx: mpsc::UnboundedSender<String>,
}

impl SslLogger {
    pub fn new(writer: LocalFileWriter) -> Self {
        let (tx, mut rx) = mpsc::unbounded_channel::<String>();  // ← 风险点
        // ...
    }
}
```

### 关联文件

| 文件 | 关联说明 |
|------|---------|
| `src/core/gateway/observe/logs/logger_factory.rs` (L75-112) | `create_sync_logger()` 构造 `SslLogger`，将 `LocalFileWriter` 传入 |
| `src/core/gateway/link_sys/providers/local_file/data_sender_impl.rs` (L29-30) | `LocalFileWriter` 内部使用 **有界** `sync_channel`，有背压机制 |
| `src/core/gateway/observe/metrics/registry.rs` (L16, L71, L157-158) | 已定义 `ssl_log_dropped` 指标，但当前 `SslLogger` 从未触发 |

### 关键对比

项目中其他日志器（TCP、TLS、UDP）使用 `AccessLogger` + `LocalFileWriter` 的模式，`LocalFileWriter` 在初始化时创建**有界** `sync_channel`：

```rust
// data_sender_impl.rs — LocalFileWriter 内部
let queue_size = self.get_queue_size();  // 默认: CPU核心数 × 10,000
let (tx, rx) = std::sync::mpsc::sync_channel::<String>(queue_size);
```

并且在 `send()` 中使用 `try_send()` + 丢弃计数：

```rust
async fn send(&self, data: String) -> Result<()> {
    if let Some(sender) = &self.sender {
        if sender.try_send(data).is_err() {
            match self.log_type {
                LogType::Ssl => global_metrics().ssl_log_dropped(),
                // ...
            }
        }
    }
    Ok(())
}
```

**SSL logger 是唯一一个绕过了 `LocalFileWriter` 内置背压机制的日志器**——它在 `LocalFileWriter` 前面加了一个无界缓冲层。

## 3. 影响分析

### 3.1 OOM 风险场景

| 场景 | 触发条件 | 严重程度 |
|------|---------|---------|
| **TLS 连接风暴** | DDoS / 合法流量突增，TLS 握手速率远超磁盘写入速率 | 高 |
| **磁盘 I/O 延迟** | NFS/EBS 抖动、日志轮转时临时阻塞、磁盘满 | 高 |
| **日志写入线程挂起** | `LocalFileWriter` 内部线程因 panic 或死锁退出，channel 接收端已 drop 但发送端 `let _ = self.tx.send(...)` 静默忽略错误 | 中 |
| **大日志体积** | `TlsConnMeta` 包含可选的 `LogBuffer`、证书信息等，单条序列化 JSON 可达数 KB | 中 |

### 3.2 内存增长估算

假设场景：10,000 TLS conn/s，每条日志 ~1 KB，磁盘写入出现 10 秒延迟：

```
堆积量 = 10,000 × 1 KB × 10 s = 100 MB
```

如果延迟持续 60 秒：

```
堆积量 = 10,000 × 1 KB × 60 s = 600 MB
```

对于边缘网关进程（通常内存预算 256MB ~ 2GB），这足以引发 OOM kill。

### 3.3 连锁效应

1. **OOM → 进程被 kill → 所有连接中断** — 网关作为流量入口，单个进程 OOM 影响面极大
2. **Kubernetes 中被 OOMKilled → 频繁重启** — 可能触发 CrashLoopBackOff
3. **没有可观测信号** — 当前无界 channel 不记录队列深度，`ssl_log_dropped` 指标永远为 0（因为 `UnboundedSender::send()` 只在接收端 drop 时失败），运维无法提前感知风险

## 4. 根因分析

SSL logger 需要一个**同步、非阻塞**的 `log_ssl()` 接口，因为它在 TLS 握手回调中被调用。项目选择了 `unbounded_channel` 来满足这个约束，这是一个合理的出发点，但缺少后续的安全防护措施。

根本问题在于：**在无界 channel 和有界的 `LocalFileWriter` 之间缺少一个带限制的中间层**。

```
TLS 回调 (同步)
    │
    ▼
 unbounded_channel  ← ← ← 无界，无背压，无监控
    │
    ▼
 LocalFileWriter.send()
    │
    ▼
 sync_channel (有界)  ← ← ← 有背压，有 dropped 计数
    │
    ▼
 磁盘写入线程
```

## 5. 推荐修复方案

### 方案 A：有界 Channel + `try_send` + 丢弃计数 (推荐)

将无界 channel 替换为有界 channel，使用 `try_send` 保持非阻塞语义，队列满时丢弃日志并计数。

```rust
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::sync::mpsc;
use crate::core::gateway::observe::metrics::global_metrics;

pub struct SslLogger {
    tx: mpsc::Sender<String>,
}

impl SslLogger {
    const DEFAULT_CHANNEL_CAPACITY: usize = 65_536;

    pub fn new(writer: LocalFileWriter) -> Self {
        Self::with_capacity(writer, Self::DEFAULT_CHANNEL_CAPACITY)
    }

    pub fn with_capacity(writer: LocalFileWriter, capacity: usize) -> Self {
        let (tx, mut rx) = mpsc::channel::<String>(capacity);

        tokio::spawn(async move {
            let mut writer = writer;
            if let Err(e) = writer.init().await {
                tracing::error!("Failed to initialize SSL log writer: {}", e);
                return;
            }
            while let Some(log) = rx.recv().await {
                let _ = writer.send(log).await;
            }
        });

        Self { tx }
    }

    #[inline]
    pub fn log(&self, entry: &TlsConnMeta) {
        let payload = serde_json::to_string(entry)
            .unwrap_or_else(|_| "{}".to_string());

        // try_send: 非阻塞，channel 满时返回 Err
        if self.tx.try_send(payload).is_err() {
            global_metrics().ssl_log_dropped();
        }
    }
}
```

**优点**：
- 内存使用有上限：`capacity × 平均日志大小`
- 复用已有的 `ssl_log_dropped` 指标，运维告警规则无需修改
- `try_send()` 保持非阻塞——TLS 回调安全不受影响
- 与 `LocalFileWriter` 内部的有界 `sync_channel` 理念一致

**容量建议**：默认 65,536 条（约 64 MB @ 1 KB/条），可通过配置覆盖。

### 方案 B：保留无界 Channel + 高水位监控告警

如果团队认为任何日志丢弃都不可接受（例如合规需求），可保留无界 channel 但加入监控。

```rust
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use tokio::sync::mpsc;

pub struct SslLogger {
    tx: mpsc::UnboundedSender<String>,
    pending: Arc<AtomicUsize>,
}

impl SslLogger {
    const HIGH_WATER_MARK: usize = 50_000;
    const CRITICAL_WATER_MARK: usize = 200_000;

    pub fn new(writer: LocalFileWriter) -> Self {
        let (tx, mut rx) = mpsc::unbounded_channel::<String>();
        let pending = Arc::new(AtomicUsize::new(0));
        let pending_clone = pending.clone();

        tokio::spawn(async move {
            let mut writer = writer;
            if let Err(e) = writer.init().await {
                tracing::error!("Failed to initialize SSL log writer: {}", e);
                return;
            }
            while let Some(log) = rx.recv().await {
                pending_clone.fetch_sub(1, Ordering::Relaxed);
                let _ = writer.send(log).await;
            }
        });

        Self { tx, pending }
    }

    #[inline]
    pub fn log(&self, entry: &TlsConnMeta) {
        let depth = self.pending.fetch_add(1, Ordering::Relaxed);

        if depth >= Self::CRITICAL_WATER_MARK {
            // 超过临界水位：丢弃并计数，防止 OOM
            self.pending.fetch_sub(1, Ordering::Relaxed);
            global_metrics().ssl_log_dropped();
            return;
        }

        if depth >= Self::HIGH_WATER_MARK && depth % 10_000 == 0 {
            tracing::warn!(
                depth,
                "SSL log channel depth exceeds high water mark"
            );
        }

        let payload = serde_json::to_string(entry)
            .unwrap_or_else(|_| "{}".to_string());
        let _ = self.tx.send(payload);
    }

    pub fn pending_count(&self) -> usize {
        self.pending.load(Ordering::Relaxed)
    }
}
```

**优点**：
- 在临界水位之前不丢弃任何日志
- 高水位告警为运维提供缓冲时间
- `pending_count()` 可暴露为 Gauge 指标

**缺点**：
- 仍然使用 `AtomicUsize` 追踪深度，有轻微性能开销
- 临界水位之前内存仍然无界，需要精心调参

### 方案 C：周期性 Channel 深度监控

无论选择方案 A 还是方案 B，都建议添加周期性深度监控。可在 metrics registry 中新增 Gauge：

```rust
// metrics/registry.rs — 新增常量和字段
pub const SSL_LOG_CHANNEL_DEPTH: &str = "edgion_ssl_log_channel_depth";

// SslLogger 中启动监控任务
fn spawn_depth_monitor(pending: Arc<AtomicUsize>) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(
            std::time::Duration::from_secs(5)
        );
        loop {
            interval.tick().await;
            let depth = pending.load(Ordering::Relaxed);
            gauge!("edgion_ssl_log_channel_depth").set(depth as f64);

            if depth > 10_000 {
                tracing::warn!(depth, "SSL log channel backlog growing");
            }
        }
    });
}
```

对应的 Prometheus 告警规则：

```yaml
groups:
  - name: edgion-ssl-log
    rules:
      - alert: SslLogChannelBacklog
        expr: edgion_ssl_log_channel_depth > 50000
        for: 1m
        labels:
          severity: warning
        annotations:
          summary: "SSL 日志 channel 堆积超过 50,000 条"
          description: >
            实例 {{ $labels.instance }} 的 SSL 日志 channel 深度为
            {{ $value }}，可能存在磁盘 I/O 瓶颈。

      - alert: SslLogChannelCritical
        expr: edgion_ssl_log_channel_depth > 200000
        for: 30s
        labels:
          severity: critical
        annotations:
          summary: "SSL 日志 channel 深度达到临界值，OOM 风险"

      - alert: SslLogDropping
        expr: rate(edgion_ssl_log_dropped_total[5m]) > 0
        for: 1m
        labels:
          severity: warning
        annotations:
          summary: "SSL 日志正在被丢弃"
```

## 6. 综合推荐

| 优先级 | 行动项 | 方案 |
|--------|--------|------|
| **P1** | 替换为有界 channel + `try_send` + 丢弃计数 | 方案 A |
| **P1** | 使 channel 容量可通过 `LogConfig` 配置 | 方案 A 扩展 |
| **P2** | 添加 channel 深度 Gauge 指标 + 周期监控 | 方案 C |
| **P2** | 配置 Prometheus 告警规则 | 方案 C |
| **P3** | 在 `logger_factory.rs` 中传递 `queue_size` 配置到 `SslLogger` | 配置一致性 |

### 配置传递示例

修改 `create_sync_logger` 以传递 queue_size：

```rust
// logger_factory.rs
pub async fn create_sync_logger(config: &LogConfig) -> Result<Option<SslLogger>> {
    // ...
    StringOutput::LocalFile(file_cfg) => {
        // ...
        let capacity = file_cfg.queue_size.unwrap_or(65_536);
        let writer = LocalFileWriter::new(writer_config)
            .with_log_type(LogType::Ssl);
        Ok(Some(SslLogger::with_capacity(writer, capacity)))
    }
}
```

## 7. 测试建议

### 7.1 单元测试：channel 满时的丢弃行为

```rust
#[tokio::test]
async fn test_ssl_logger_drops_when_full() {
    // 使用极小容量模拟背压
    let writer = LocalFileWriter::with_path("/dev/null");
    let logger = SslLogger::with_capacity(writer, 2);

    let entry = TlsConnMeta::default();

    // 快速发送超过容量的日志
    for _ in 0..100 {
        logger.log(&entry);
    }

    // 验证 ssl_log_dropped 指标被递增
    // (需要先 reset metrics 或使用 test-specific metrics)
    tokio::time::sleep(Duration::from_millis(100)).await;
}
```

### 7.2 集成测试：高 TLS 连接速率下的内存稳定性

```rust
#[tokio::test]
async fn test_ssl_logger_memory_bounded_under_pressure() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("ssl.log");
    let writer = LocalFileWriter::with_path(path.to_str().unwrap());
    let logger = SslLogger::with_capacity(writer, 1_000);

    let entry = TlsConnMeta::default();

    // 模拟 100,000 次 TLS 握手
    for _ in 0..100_000 {
        logger.log(&entry);
    }

    // 给消费者时间处理
    tokio::time::sleep(Duration::from_secs(2)).await;

    // 进程 RSS 应保持在合理范围内（不会像无界 channel 那样线性增长）
    // 可用 /proc/self/status 或 jemalloc stats 验证
}
```

### 7.3 模拟磁盘 I/O 慢速场景

```rust
#[tokio::test]
async fn test_ssl_logger_slow_writer() {
    // 使用自定义 SlowWriter 模拟慢磁盘
    // SlowWriter 的 send() 中加入 tokio::time::sleep(Duration::from_millis(100))
    // 验证 channel 深度不会无限增长，且 dropped 计数正确递增
}
```

### 7.4 压力测试脚本

```bash
# 使用 openssl s_client 进行高频 TLS 握手
for i in $(seq 1 10000); do
    openssl s_client -connect localhost:8443 -servername test.example.com \
        </dev/null >/dev/null 2>&1 &
done
wait

# 检查指标
curl -s http://localhost:9090/metrics | grep ssl_log
# 预期:
#   edgion_ssl_log_dropped_total > 0 (如果达到容量上限)
#   edgion_ssl_log_channel_depth < capacity (如果加了深度监控)
```

## 8. 总结

当前 `SslLogger` 的无界 channel 设计在正常流量下工作良好，但在极端场景下缺乏安全防护。核心矛盾是：**TLS 回调的非阻塞约束** vs. **内存安全**。

推荐的方案 A（有界 channel + `try_send`）以最小改动解决了这个矛盾：
- 保持 `log_ssl()` 非阻塞
- 内存使用有上限
- 复用已有的 `ssl_log_dropped` 指标（该指标已在 metrics registry 中定义，但当前永远为 0）
- 与项目中其他日志器的设计理念一致

这是一个**低风险、高回报**的改进。
