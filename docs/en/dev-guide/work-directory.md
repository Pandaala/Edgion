# Work Directory Design and Path Management

This document introduces Edgion's Work Directory design and path management optimization.

## Overview

The Work Directory is Edgion's unified working directory, used to manage all runtime files including configuration, logs, runtime state, etc.

### Design Goals

1. **Unified path management**: All relative paths are based on a unified `work_dir`
2. **Flexible configuration**: Supports multiple configuration methods for different deployment scenarios
3. **Clear priority**: CLI > Environment variable > Config file > Default
4. **Auto validation**: Automatically checks directory permissions and creates subdirectories at startup

## Directory Structure

Standard work directory layout:

```
work_dir/
├── config/          # Configuration files
│   ├── edgion-gateway.toml
│   └── edgion-controller.toml
├── logs/            # Log files
│   ├── edgion_access.log
│   ├── ssl.log
│   └── edgion-gateway.log
└── runtime/         # Runtime state
    └── (future use)
```

## Configuration Methods

### Priority Order

1. **CLI parameter** (highest priority)
   ```bash
   ./edgion-gateway --work-dir /usr/local/edgion
   ```

2. **Environment variable**
   ```bash
   export EDGION_WORK_DIR=/usr/local/edgion
   ./edgion-gateway
   ```

3. **Config file**
   ```toml
   # edgion-gateway.toml
   work_dir = "/usr/local/edgion"
   ```

4. **Default** (lowest priority)
   - Defaults to current directory `.`

### Configuration for Different Scenarios

#### Development Environment
```toml
work_dir = "."  # Current directory
```
Or simply omit it to use the default.

#### Production Environment
```toml
work_dir = "/usr/local/edgion"
```

#### Container Environment
```toml
work_dir = "/usr/local/edgion"
```
Or via environment variable:
```dockerfile
ENV EDGION_WORK_DIR=/usr/local/edgion
```

## Path Resolution

### WorkDir API

```rust
use crate::types::{init_work_dir, work_dir};

// 1. Initialize (at application startup)
init_work_dir(base_path)?;

// 2. Get subdirectories
let logs_dir = work_dir().logs();     // work_dir/logs
let config_dir = work_dir().config(); // work_dir/config
let runtime_dir = work_dir().runtime(); // work_dir/runtime

// 3. Resolve relative/absolute paths
let log_path = work_dir().resolve("logs/access.log");
// Relative path -> work_dir/logs/access.log
// Absolute path -> Unchanged
```

### Path Resolution Rules

| Input Path | work_dir | Resolved Result |
|-----------|---------|----------------|
| `logs/access.log` | `/usr/local/edgion` | `/usr/local/edgion/logs/access.log` |
| `/var/log/edgion.log` | `/usr/local/edgion` | `/var/log/edgion.log` (absolute path preserved) |
| `config/gateway.toml` | `.` | `./config/gateway.toml` |

### Implementation Details

```rust
impl WorkDir {
    pub fn resolve(&self, path: impl AsRef<Path>) -> PathBuf {
        let path = path.as_ref();
        if path.is_absolute() {
            path.to_path_buf()  // Return absolute path directly
        } else {
            self.base.join(path)  // Join relative path with base
        }
    }
}
```

## Initialization Flow

### 1. Determine work_dir

```rust
// src/core/cli/edgion_gateway/mod.rs
let base_work_dir = self.config.work_dir.clone()
    .or_else(|| std::env::var("EDGION_WORK_DIR").ok().map(PathBuf::from))
    .unwrap_or_else(|| PathBuf::from("."));

init_work_dir(base_work_dir)?;
```

### 2. Validate Directory

```rust
// src/types/work_dir.rs
impl WorkDir {
    pub fn validate(&self) -> anyhow::Result<()> {
        // 1. Check if base directory exists, create if not
        if !self.base.exists() {
            std::fs::create_dir_all(&self.base)?;
        }
        
        // 2. Check if it is a directory
        if !self.base.is_dir() {
            return Err(anyhow!("Not a directory"));
        }
        
        // 3. Check writability
        let test_file = self.base.join(".edgion_write_test");
        std::fs::write(&test_file, b"test")?;
        std::fs::remove_file(&test_file)?;
        
        // 4. Create subdirectories
        for dir in [&self.logs, &self.runtime, &self.config] {
            std::fs::create_dir_all(dir)?;
        }
        
        Ok(())
    }
}
```

### 3. Use work_dir

```rust
// Log system initialization
let log_path = work_dir().resolve("logs/access.log");
let writer = LocalFileWriter::new(LocalFileWriterConfig {
    path: "logs/access.log".to_string(),  // Relative path
    ..Default::default()
});

// writer internally calls work_dir().resolve() to resolve the full path
```

## Migration Guide

### Migrating from prefix_dir to work_dir

#### Old Code (Deprecated)
```rust
use crate::types::global_def::prefix_dir;

let log_path = prefix_dir().join("logs/access.log");
```

#### New Code
```rust
use crate::types::work_dir;

let log_path = work_dir().resolve("logs/access.log");
// Or
let log_path = work_dir().logs().join("access.log");
```

### Configuration File Migration

#### Old Configuration (Deprecated)
```toml
# edgion-gateway.toml
prefix_dir = "/usr/local/edgion"
```

#### New Configuration
```toml
# edgion-gateway.toml
work_dir = "/usr/local/edgion"
```

## Testing

### Unit Tests

```rust
#[test]
fn test_work_dir_resolve() {
    let temp = tempfile::tempdir().unwrap();
    let wd = WorkDir::new(temp.path().to_path_buf()).unwrap();
    
    // Test relative path
    let relative = wd.resolve("logs/access.log");
    assert!(relative.starts_with(temp.path()));
    
    // Test absolute path
    let absolute = wd.resolve("/var/log/test.log");
    assert_eq!(absolute, PathBuf::from("/var/log/test.log"));
}
```

### Integration Tests

```bash
# Test different configuration methods
mkdir -p /tmp/edgion-test-workdir

# CLI has highest priority
./target/debug/edgion-gateway \
    --work-dir /tmp/edgion-test-workdir \
    --help > /dev/null 2>&1

ls -la /tmp/edgion-test-workdir/
# Should see config/, logs/, runtime/ directories
```

## Troubleshooting

### Permission Error

**Symptom**:
```
Error: Work directory /usr/local/edgion is not writable
```

**Solution**:
```bash
# Check permissions
ls -ld /usr/local/edgion

# Fix permissions
sudo chown -R edgion:edgion /usr/local/edgion
sudo chmod 755 /usr/local/edgion
```

### Path Does Not Exist

**Symptom**:
```
Error: Cannot create work_dir /nonexistent/path
```

**Solution**:
1. Check if the parent directory exists
2. Check for create permissions
3. Use an existing directory or create the parent directory

### Relative Path Issues

**Symptom**: Log files appearing in the wrong location

**Cause**: Not using `work_dir().resolve()` correctly

**Solution**:
```rust
// Wrong: Using relative path directly
let path = PathBuf::from("logs/access.log");

// Correct: Resolve through work_dir
let path = work_dir().resolve("logs/access.log");
```

## Best Practices

### 1. Use Absolute Paths in Production

```toml
# Recommended
work_dir = "/usr/local/edgion"

# Not recommended (depends on startup location)
work_dir = "."
```

### 2. Use Environment Variables in Containers

```dockerfile
ENV EDGION_WORK_DIR=/usr/local/edgion
WORKDIR /usr/local/edgion
```

### 3. Use Relative Paths for Log Files

```toml
# Recommended (relative to work_dir)
[access_log.output.localFile]
path = "logs/access.log"

# Not recommended (absolute path breaks work_dir unified management)
path = "/var/log/edgion/access.log"
```

### 4. Validate Before Startup

```bash
# Check if work_dir is writable
test -w /usr/local/edgion || exit 1

# Create necessary subdirectories
mkdir -p /usr/local/edgion/{config,logs,runtime}
```

## Related Files

### Core Implementation
- `src/types/work_dir.rs` - WorkDir implementation
- `src/types/global_def.rs` - Global constants (DEFAULT_WORK_DIR)
- `src/types/mod.rs` - Export work_dir API

### Usage Locations
- `src/core/cli/edgion_gateway/mod.rs` - Gateway initialization
- `src/core/cli/edgion_controller/mod.rs` - Controller initialization
- `src/core/link_sys/local_file/mod.rs` - Log file path resolution
- `src/core/observe/ssl_log.rs` - SSL log path

### Configuration Files
- `config/edgion-gateway.toml` - Gateway configuration
- `config/edgion-controller.toml` - Controller configuration

## References

### Related Design Documents
- [Logging System Architecture](./logging-system.md) - Log file path management
- [Architecture Overview](./architecture-overview.md) - Overall system design

### External References
- [XDG Base Directory Specification](https://specifications.freedesktop.org/basedir-spec/basedir-spec-latest.html)
- [Filesystem Hierarchy Standard](https://refspecs.linuxfoundation.org/FHS_3.0/fhs/index.html)

---

**Last updated**: 2025-01-05  
**Version**: Edgion v0.1.0
