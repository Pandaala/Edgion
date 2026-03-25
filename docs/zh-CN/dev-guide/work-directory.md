# Work Directory 设计与路径管理

本文档介绍 Edgion 的工作目录（Work Directory）设计和路径管理优化。

## 概述

Work Directory 是 Edgion 的统一工作目录，用于管理所有运行时文件，包括配置、日志、运行时状态等。

### 设计目标

1. **统一路径管理**：所有相对路径基于统一的 `work_dir`
2. **灵活配置**：支持多种配置方式，适应不同部署场景
3. **清晰优先级**：CLI > 环境变量 > 配置文件 > 默认值
4. **自动验证**：启动时自动检查目录权限和创建子目录

## 目录结构

标准的 work directory 布局：

```
work_dir/
├── config/          # 配置文件
│   ├── edgion-gateway.toml
│   └── edgion-controller.toml
├── logs/            # 日志文件
│   ├── edgion_access.log
│   ├── ssl.log
│   └── edgion-gateway.log
└── runtime/         # 运行时状态
    └── (future use)
```

## 配置方式

### 优先级顺序

1. **CLI 参数**（最高优先级）
   ```bash
   ./edgion-gateway --work-dir /usr/local/edgion
   ```

2. **环境变量**
   ```bash
   export EDGION_WORK_DIR=/usr/local/edgion
   ./edgion-gateway
   ```

3. **配置文件**
   ```toml
   # edgion-gateway.toml
   work_dir = "/usr/local/edgion"
   ```

4. **默认值**（最低优先级）
   - 默认为当前目录 `.`

### 不同场景的配置

#### 开发环境
```toml
work_dir = "."  # 当前目录
```
或直接省略，使用默认值。

#### 生产环境
```toml
work_dir = "/usr/local/edgion"
```

#### 容器环境
```toml
work_dir = "/usr/local/edgion"
```
或通过环境变量：
```dockerfile
ENV EDGION_WORK_DIR=/usr/local/edgion
```

## 路径解析

### WorkDir API

```rust
use crate::types::{init_work_dir, work_dir};

// 1. 初始化（应用启动时）
init_work_dir(base_path)?;

// 2. 获取子目录
let logs_dir = work_dir().logs();     // work_dir/logs
let config_dir = work_dir().config(); // work_dir/config
let runtime_dir = work_dir().runtime(); // work_dir/runtime

// 3. 解析相对/绝对路径
let log_path = work_dir().resolve("logs/edgion_access.log");
// 相对路径 -> work_dir/logs/edgion_access.log
// 绝对路径 -> 保持不变
```

### 路径解析规则

| 输入路径 | work_dir | 解析结果 |
|---------|---------|---------|
| `logs/edgion_access.log` | `/usr/local/edgion` | `/usr/local/edgion/logs/edgion_access.log` |
| `/var/log/edgion.log` | `/usr/local/edgion` | `/var/log/edgion.log`（保持绝对路径） |
| `config/gateway.toml` | `.` | `./config/gateway.toml` |

### 实现细节

```rust
impl WorkDir {
    pub fn resolve(&self, path: impl AsRef<Path>) -> PathBuf {
        let path = path.as_ref();
        if path.is_absolute() {
            path.to_path_buf()  // 绝对路径直接返回
        } else {
            self.base.join(path)  // 相对路径拼接 base
        }
    }
}
```

## 初始化流程

### 1. 确定 work_dir

```rust
// src/core/cli/edgion_gateway/mod.rs
let base_work_dir = self.config.work_dir.clone()
    .or_else(|| std::env::var("EDGION_WORK_DIR").ok().map(PathBuf::from))
    .unwrap_or_else(|| PathBuf::from("."));

init_work_dir(base_work_dir)?;
```

### 2. 验证目录

```rust
// src/types/work_dir.rs
impl WorkDir {
    pub fn validate(&self) -> anyhow::Result<()> {
        // 1. 检查 base 目录是否存在，不存在则创建
        if !self.base.exists() {
            std::fs::create_dir_all(&self.base)?;
        }
        
        // 2. 检查是否为目录
        if !self.base.is_dir() {
            return Err(anyhow!("Not a directory"));
        }
        
        // 3. 检查可写性
        let test_file = self.base.join(".edgion_write_test");
        std::fs::write(&test_file, b"test")?;
        std::fs::remove_file(&test_file)?;
        
        // 4. 创建子目录
        for dir in [&self.logs, &self.runtime, &self.config] {
            std::fs::create_dir_all(dir)?;
        }
        
        Ok(())
    }
}
```

### 3. 使用 work_dir

```rust
// 日志系统初始化
let log_path = work_dir().resolve("logs/edgion_access.log");
let writer = LocalFileWriter::new(LocalFileWriterConfig {
    path: "logs/edgion_access.log".to_string(),  // 相对路径
    ..Default::default()
});

// writer 内部会调用 work_dir().resolve() 解析完整路径
```

## 迁移指南

### 从 prefix_dir 迁移到 work_dir

#### 旧代码（已弃用）
```rust
use crate::types::global_def::prefix_dir;

let log_path = prefix_dir().join("logs/edgion_access.log");
```

#### 新代码
```rust
use crate::types::work_dir;

let log_path = work_dir().resolve("logs/edgion_access.log");
// 或
let log_path = work_dir().logs().join("edgion_access.log");
```

### 配置文件迁移

#### 旧配置（已弃用）
```toml
# edgion-gateway.toml
prefix_dir = "/usr/local/edgion"
```

#### 新配置
```toml
# edgion-gateway.toml
work_dir = "/usr/local/edgion"
```

## 测试

### 单元测试

```rust
#[test]
fn test_work_dir_resolve() {
    let temp = tempfile::tempdir().unwrap();
    let wd = WorkDir::new(temp.path().to_path_buf()).unwrap();
    
    // 测试相对路径
    let relative = wd.resolve("logs/edgion_access.log");
    assert!(relative.starts_with(temp.path()));
    
    // 测试绝对路径
    let absolute = wd.resolve("/var/log/test.log");
    assert_eq!(absolute, PathBuf::from("/var/log/test.log"));
}
```

### 集成测试

```bash
# 测试不同配置方式
mkdir -p /tmp/edgion-test-workdir

# CLI 优先级最高
./target/debug/edgion-gateway \
    --work-dir /tmp/edgion-test-workdir \
    --help > /dev/null 2>&1

ls -la /tmp/edgion-test-workdir/
# 应该看到 config/, logs/, runtime/ 三个目录
```

## 故障排查

### 权限错误

**症状**：
```
Error: Work directory /usr/local/edgion is not writable
```

**解决**：
```bash
# 检查权限
ls -ld /usr/local/edgion

# 修复权限
sudo chown -R edgion:edgion /usr/local/edgion
sudo chmod 755 /usr/local/edgion
```

### 路径不存在

**症状**：
```
Error: Cannot create work_dir /nonexistent/path
```

**解决**：
1. 检查父目录是否存在
2. 检查是否有创建权限
3. 使用现有目录或创建父目录

### 相对路径问题

**症状**：日志文件出现在错误的位置

**原因**：未正确使用 `work_dir().resolve()`

**解决**：
```rust
// ❌ 错误：直接使用相对路径
let path = PathBuf::from("logs/edgion_access.log");

// ✅ 正确：通过 work_dir 解析
let path = work_dir().resolve("logs/edgion_access.log");
```

## 最佳实践

### 1. 生产环境使用绝对路径

```toml
# 推荐
work_dir = "/usr/local/edgion"

# 不推荐（依赖启动位置）
work_dir = "."
```

### 2. 容器环境使用环境变量

```dockerfile
ENV EDGION_WORK_DIR=/usr/local/edgion
WORKDIR /usr/local/edgion
```

### 3. 日志文件使用相对路径

```toml
# 推荐（相对于 work_dir）
[access_log.output.localFile]
path = "logs/edgion_access.log"

# 不推荐（绝对路径破坏了 work_dir 的统一管理）
path = "/var/log/edgion/edgion_access.log"
```

### 4. 启动前验证

```bash
# 检查 work_dir 是否可写
test -w /usr/local/edgion || exit 1

# 创建必要的子目录
mkdir -p /usr/local/edgion/{config,logs,runtime}
```

## 相关文件

### 核心实现
- `src/types/work_dir.rs` - WorkDir 实现
- `src/types/global_def.rs` - 全局常量（DEFAULT_WORK_DIR）
- `src/types/mod.rs` - 导出 work_dir API

### 使用位置
- `src/core/cli/edgion_gateway/mod.rs` - Gateway 初始化
- `src/core/cli/edgion_controller/mod.rs` - Controller 初始化
- `src/core/link_sys/local_file/mod.rs` - 日志文件路径解析
- `src/core/observe/ssl_log.rs` - SSL 日志路径

### 配置文件
- `config/edgion-gateway.toml` - Gateway 配置
- `config/edgion-controller.toml` - Controller 配置

## 参考资料

### 相关设计文档
- [日志系统架构](./logging-system.md) - 日志文件路径管理
- [架构概览](./architecture-overview.md) - 整体系统设计

### 外部参考
- [XDG Base Directory Specification](https://specifications.freedesktop.org/basedir-spec/basedir-spec-latest.html)
- [Filesystem Hierarchy Standard](https://refspecs.linuxfoundation.org/FHS_3.0/fhs/index.html)

---

**最后更新**：2025-01-05  
**版本**：Edgion v0.1.0
