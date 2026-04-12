# Feature Flags 参考

用于这些任务：

- 切换 TLS 后端（boringssl / rustls）
- 切换内存分配器（jemalloc / mimalloc / system）
- 理解各 feature flag 的互斥关系和编译影响
- CI/CD 中选择正确的 feature 组合

## Feature Flags 一览

Edgion 支持两组互斥 feature flags，构建时必须每组选一个：

**TLS 后端**（二选一）：

| Flag | 说明 | 默认 |
|------|------|------|
| `boringssl` | Google BoringSSL，生产推荐 | 是 |
| `rustls` | 纯 Rust TLS 实现，实验性 | 否 |

**内存分配器**（三选一）：

| Flag | 说明 | 默认 |
|------|------|------|
| `allocator-jemalloc` | jemalloc，推荐 | 是 |
| `allocator-mimalloc` | mimalloc，备选 | 否 |
| `allocator-system` | 系统默认分配器 | 否 |

## 常用构建命令

```bash
# 默认：jemalloc + boringssl
cargo build

# rustls 实验构建
cargo build --no-default-features --features "allocator-jemalloc,rustls"

# mimalloc + boringssl
cargo build --no-default-features --features "allocator-mimalloc,boringssl"
```

## 注意事项

- 两组 flag 均为互斥，不能同时启用同组的多个 flag
- `rustls` 目前仍处于实验阶段，Gateway TLS 运行时主要依赖 `boringssl`
- CI 默认使用 `boringssl` + `allocator-jemalloc` 组合

## 参考文档

- 详细 Feature Flag 语义：[../02-features/01-binary-and-deployment/02-feature-flags.md](../02-features/01-binary-and-deployment/02-feature-flags.md)
