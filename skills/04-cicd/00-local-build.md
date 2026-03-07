---
name: local-build
description: Local build commands, feature combinations, and common build issues.
---
# 本地编译

> 本地编译命令、Cargo Feature 组合、常见编译问题。
>
> **TODO (2026-02-25): P1**
> - [ ] 开发模式 vs 发布模式编译命令详解
> - [ ] Feature 组合矩阵（TLS 后端 × 内存分配器）
> - [ ] 各平台编译前置依赖（macOS/Linux：BoringSSL 需要 cmake/go、OpenSSL 需要 libssl-dev）
> - [ ] 常见编译错误及解决方案（BoringSSL 链接错误、protobuf 编译错误、交叉编译配置）
> - [ ] 编译产物位置与运行方式

## 快速参考

```bash
# 开发模式（快速编译，含 debug symbols）
cargo build

# 发布模式（优化编译）
cargo build --release

# 代码检查（不编译产物，最快）
cargo check

# Lint 检查
cargo clippy

# 格式检查
cargo fmt --check

# 切换 TLS 后端
cargo build --release --no-default-features --features "allocator-jemalloc,openssl"
cargo build --release --no-default-features --features "allocator-jemalloc,rustls"

# 切换内存分配器
cargo build --release --no-default-features --features "allocator-mimalloc,boringssl"
```

## Feature 速查

详见 [cicd/SKILL.md](SKILL.md) 中的 Cargo Features 速查表。
