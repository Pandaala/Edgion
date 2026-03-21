---
name: feature-flags
description: Cargo Feature Flags 参考：allocator、TLS backend、测试选项及构建组合。
---

# Feature Flags

## 默认构建

```bash
cargo build    # 等价于 --features allocator-jemalloc,boringssl
```

## Feature 矩阵

| Feature | 类型 | 说明 |
|---------|------|------|
| `allocator-jemalloc` | 互斥组 A | jemalloc 内存分配器（默认，推荐生产） |
| `allocator-mimalloc` | 互斥组 A | mimalloc 内存分配器 |
| — | 互斥组 A | 系统分配器（不指定任一 allocator 时） |
| `boringssl` | 互斥组 B | BoringSSL TLS 后端（默认，推荐生产） |
| `openssl` | 互斥组 B | OpenSSL TLS 后端 |
| `rustls` | 互斥组 B | rustls TLS 后端（**注意：不完全对等**） |
| `legacy_route_tests` | 独立 | 保留/占位，当前无实际效果 |

### 互斥规则

- **组 A（Allocator）**：`jemalloc` / `mimalloc` / 系统，三选一
- **组 B（TLS Backend）**：`boringssl` / `openssl` / `rustls`，三选一
- 跨组可自由组合

### rustls 注意事项

`rustls` 不代表完整的 Gateway TLS 对等功能。代码中仍有 `#[cfg(feature = "boringssl")]` 和 `#[cfg(feature = "openssl")]` 保护的分支，rustls 路径可能缺少部分高级 TLS 功能（如 OCSP stapling、session ticket rotation）。

## 构建示例

```bash
# 默认（推荐）
cargo build --release

# mimalloc + openssl
cargo build --release --no-default-features --features allocator-mimalloc,openssl

# 系统分配器 + boringssl
cargo build --release --no-default-features --features boringssl

# rustls（开发/测试）
cargo build --no-default-features --features rustls
```

详细矩阵见 [references/feature-flags-matrix.md](references/feature-flags-matrix.md)。
