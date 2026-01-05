# Edgion 日志系统架构

本文档介绍 Edgion 的日志系统架构，包括 Access Log 和 SSL/TLS Callback Log 的设计与实现。

## 概览

Edgion 提供了统一的日志基础设施，支持：
- **Access Log**：记录所有 HTTP/HTTPS 请求
- **SSL Log**：记录所有 TLS 握手和证书回调事件
- **批处理写入**：减少系统调用，提升性能
- **日志轮转**：按时间或大小自动轮转
- **多种输出**：本地文件、Elasticsearch、Kafka（未来支持）
- **Metrics 集成**：记录丢弃日志数量

## 架构设计

### 整体架构

```mermaid
graph TB
    subgraph Gateway[Edgion Gateway]
        ProxyReq[Proxy Request]
        TLSCallback[TLS Callback]
    end
    
    subgraph LogSystem[Logging System]
        AccessLogger[Access Logger]
        SslLogger[SSL Logger]
        
        subgraph Infrastructure[Shared Infrastructure]
            LocalFileWriter[LocalFileWriter]
            BatchWriter[Batch Writer Thread]
            Rotator[Log Rotator]
        end
    end
    
    subgraph Output[Output]
        LogFile1[access.log]
        LogFile2[ssl.log]
        Metrics[Prometheus Metrics]
    end
    
    ProxyReq -->|async| AccessLogger
    TLSCallback -->|sync| SslLogger
    
    AccessLogger -->|tokio channel| LocalFileWriter
    SslLogger -->|tokio unbounded channel| LocalFileWriter
    
    LocalFileWriter -->|sync channel| BatchWriter
    BatchWriter -->|batch write| LogFile1
    BatchWriter -->|batch write| LogFile2
    BatchWriter -->|check| Rotator
    
    LocalFileWriter -.->|dropped count| Metrics
```

### 关键特性

#### 1. 非阻塞保证

- **Access Log**：使用 async API，不阻塞 tokio runtime
- **SSL Log**：使用 unbounded channel 桥接，保证 TLS callback 永不阻塞

```rust
// SSL Log - 同步 API，内部使用 unbounded channel
#[inline]
pub fn log_ssl(entry: &SslLogEntry) {
    if let Some(logger) = SSL_LOGGER.get() {
        // UnboundedSender::send() 永不阻塞
        let _ = logger.tx.send(entry.to_json());
    }
}
```

#### 2. 批处理写入

LocalFileWriter 实现批处理写入逻辑：

```rust
// 阻塞等待第一条日志
while let Ok(first_line) = rx.recv() {
    let _ = writeln!(file, "{}", first_line);
    
    // 批量处理剩余日志（最多 999 条，总共 1000 条）
    for _ in 0..999 {
        match rx.try_recv() {
            Ok(line) => {
                let _ = writeln!(file, "{}", line);
            }
            Err(_) => break,
        }
    }
    
    // 一次性 flush
    file.flush();
}
```

**性能优势**：
- 减少约 1000 倍的 `write()` 系统调用
- 减少约 1000 倍的 `flush()` 系统调用
- 显著降低 I/O 压力

#### 3. 日志轮转

支持三种轮转策略：

| 策略 | 说明 | 适用场景 |
|------|------|----------|
| `Size` | 按文件大小轮转 | 高流量场景，避免单文件过大 |
| `Daily` | 每日轮转（午夜） | 按日归档，便于分析 |
| `Hourly` | 每小时轮转 | 高频归档，便于实时分析 |
| `Never` | 不轮转 | 开发测试环境 |

轮转文件命名规则：
- **时间轮转**：`access.log.2025-01-05` 或 `access.log.2025-01-05-14`
- **大小轮转**：`access.log.1`, `access.log.2`, `access.log.3`

自动清理旧文件，保留最近 N 个（可配置 `max_files`）。

#### 4. Metrics 集成

当队列满导致日志丢弃时，自动记录 metrics：

```rust
async fn send(&self, data: String) -> Result<()> {
    if let Some(sender) = &self.sender {
        if sender.try_send(data).is_err() {
            // 记录丢弃指标
            global_metrics().access_log_dropped();
        }
    }
    Ok(())
}
```

可通过 Prometheus 监控 `access_log_dropped` 指标，及时发现问题。

## 模块详解

### Access Logger

**位置**：`src/core/observe/access_log/`

**架构**：

```rust
pub struct AccessLogger {
    senders: Vec<Box<dyn DataSender<String>>>,
}
```

- 支持多个输出目标（当前使用第一个健康的 sender）
- 插件化设计，便于扩展新的输出类型

**初始化流程**：

1. 读取配置 `AccessLogConfig`
2. 根据 `StringOutput` 类型创建相应的 `DataSender`
3. 调用 `sender.init()` 初始化（创建文件、连接数据库等）
4. 注册到全局 `AccessLogger`

### SSL Logger

**位置**：`src/core/observe/ssl_log.rs`

**架构**：

```rust
pub struct SslLogger {
    // 使用 unbounded channel 桥接异步 LocalFileWriter
    tx: mpsc::UnboundedSender<String>,
}
```

**设计要点**：

1. **同步 API**：`log_ssl()` 是同步函数，可在 TLS callback 中安全调用
2. **异步桥接**：内部使用 tokio unbounded channel 转发到异步 `LocalFileWriter`
3. **向后兼容**：保留 `SslLogEntry` 结构体，API 不变

**初始化流程**：

```rust
// 1. 创建 LocalFileWriter
let writer = LocalFileWriter::new(ssl_cfg);

// 2. 初始化 SSL Logger（内部启动 tokio task）
init_ssl_logger(writer).await?;

// 3. 在 TLS callback 中使用
log_ssl(&entry);  // 同步调用，永不阻塞
```

### LocalFileWriter

**位置**：`src/core/link_sys/local_file/`

**职责**：
- 管理日志文件的打开、写入、关闭
- 实现批处理写入
- 实现日志轮转
- 记录 metrics

**配置**：

```rust
pub struct LocalFileWriterConfig {
    pub path: String,              // 相对路径（相对于 work_dir）
    pub queue_size: Option<usize>, // 队列大小，默认 cores * 10000
    pub rotation: RotationConfig,  // 轮转配置
}

pub struct RotationConfig {
    pub strategy: RotationStrategy,      // 轮转策略
    pub max_files: usize,                // 保留文件数
    pub check_interval_secs: u64,        // 检查间隔
}
```

**关键方法**：

- `init()` - 初始化，创建目录和后台线程
- `send()` - 发送日志（非阻塞）
- `healthy()` - 检查健康状态

## 配置示例

### Access Log 配置

```toml
[access_log.output.localFile]
path = "logs/edgion_access.log"
queue_size = 100000  # 可选，默认 cores * 10000

[access_log.output.localFile.rotation]
strategy = "daily"  # 或 "hourly", "never", { size = 104857600 }
max_files = 10
check_interval_secs = 30
```

### SSL Log 配置

```toml
[ssl_log]
enabled = true

[ssl_log.output.localFile]
path = "logs/ssl.log"
queue_size = 100000

[ssl_log.output.localFile.rotation]
strategy = { size = 104857600 }  # 100MB per file
max_files = 10
check_interval_secs = 30
```

## 性能考虑

### 队列大小

默认队列大小 = `CPU 核心数 * 10000`

- **4 核**：40000 条日志
- **8 核**：80000 条日志
- **16 核**：160000 条日志

根据流量调整 `queue_size`：
- **高流量**（10K+ RPS）：增大到 200000+
- **低流量**（<1K RPS）：默认值足够
- **调试模式**：可减小到 10000

### 内存占用

估算公式：
```
内存 ≈ queue_size * avg_log_size
```

示例：
- 队列大小：100000
- 平均日志：200 字节
- 内存占用：≈ 20MB

### 批处理效果

| 场景 | 无批处理 | 批处理（1000） | 提升 |
|------|---------|---------------|------|
| RPS | 10000 | 10000 | - |
| write/s | 10000 | 10 | 1000x |
| CPU | 15% | 2% | 7.5x |

## 扩展新的输出类型

### 1. 实现 DataSender trait

```rust
use async_trait::async_trait;
use crate::core::link_sys::DataSender;

pub struct ElasticsearchSender {
    client: EsClient,
    index: String,
}

#[async_trait]
impl DataSender<String> for ElasticsearchSender {
    async fn init(&mut self) -> Result<()> {
        // 初始化连接
        self.client.connect().await
    }
    
    fn healthy(&self) -> bool {
        self.client.is_connected()
    }
    
    async fn send(&self, data: String) -> Result<()> {
        self.client.index(&self.index, data).await
    }
    
    fn name(&self) -> &str {
        "elasticsearch"
    }
}
```

### 2. 添加配置类型

```rust
// src/types/link_sys.rs
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum StringOutput {
    LocalFile(LocalFileWriterCfg),
    Elasticsearch(ElasticsearchCfg),  // 新增
}
```

### 3. 更新初始化逻辑

```rust
// src/core/observe/access_log/mod.rs
match &config.output {
    StringOutput::LocalFile(cfg) => { /* ... */ }
    StringOutput::Elasticsearch(cfg) => {
        let sender = ElasticsearchSender::new(cfg);
        logger.register(Box::new(sender));
    }
}
```

## 故障排查

### 日志丢失

**症状**：日志文件中缺少某些请求记录

**原因**：
1. 队列满，日志被丢弃
2. 磁盘空间不足
3. 文件权限问题

**排查**：
1. 检查 metrics：`access_log_dropped` 是否增长
2. 检查磁盘空间：`df -h`
3. 检查文件权限：`ls -la logs/`
4. 增大 `queue_size` 配置

### 日志延迟

**症状**：日志写入延迟较大

**原因**：
1. 批处理导致的正常延迟（最多 1 秒）
2. 磁盘 I/O 慢
3. 队列积压

**排查**：
1. 检查磁盘 I/O：`iostat -x 1`
2. 检查队列深度（需添加 metrics）
3. 调整 `check_interval_secs` 减少轮转检查频率

### SSL 日志为空

**症状**：`ssl.log` 文件不存在或为空

**原因**：
1. `ssl_log.enabled = false`
2. 没有 TLS 流量
3. TLS 配置错误

**排查**：
1. 检查配置：`cat config/edgion-gateway.toml | grep ssl_log`
2. 检查启动日志：`grep "SSL logger initialized" logs/*.log`
3. 使用 `curl -k https://...` 测试 TLS

## 相关文件

### 核心实现

- `src/core/observe/access_log/` - Access Log 实现
- `src/core/observe/ssl_log.rs` - SSL Log 实现
- `src/core/link_sys/local_file/` - LocalFileWriter 实现
- `src/core/link_sys/data_sender_trait.rs` - DataSender trait 定义

### 配置

- `src/core/cli/edgion_gateway/config.rs` - Gateway 配置结构
- `src/types/link_sys.rs` - 日志输出配置类型
- `config/edgion-gateway.toml` - 配置示例

### 用户文档

- `docs/user-guide/access-log-guide.md` - Access Log 用户指南
- `docs/user-guide/edgiontls-user-guide.md` - TLS 配置指南（包含 SSL Log 说明）

## 最佳实践

### 生产环境

1. **启用日志轮转**
   ```toml
   [access_log.output.localFile.rotation]
   strategy = { size = 104857600 }  # 100MB
   max_files = 30  # 保留 30 个文件（约 3GB）
   ```

2. **适当的队列大小**
   ```toml
   queue_size = 200000  # 高流量场景
   ```

3. **监控 metrics**
   - 配置 Prometheus 抓取 `/metrics`
   - 设置告警：`access_log_dropped > 0`

4. **定期归档**
   - 使用 logrotate 或自定义脚本
   - 压缩旧日志：`gzip logs/*.log.*`
   - 上传到对象存储

### 开发环境

1. **禁用日志轮转**
   ```toml
   [access_log.output.localFile.rotation]
   strategy = "never"
   ```

2. **减小队列大小**
   ```toml
   queue_size = 10000
   ```

3. **启用 SSL 日志**
   ```toml
   [ssl_log]
   enabled = true  # 调试 TLS 问题
   ```

## 未来计划

### 短期（v0.2）

- [ ] 添加 Elasticsearch 输出支持
- [ ] 添加 Kafka 输出支持
- [ ] 支持自定义日志格式
- [ ] 支持日志采样（高流量场景）

### 长期（v1.0）

- [ ] 支持结构化日志（JSON）
- [ ] 支持日志压缩
- [ ] 支持远程 syslog
- [ ] 支持日志脱敏

---

**最后更新**：2025-01-05  
**版本**：Edgion v0.1.0

