---
name: feature-flags
description: Cargo Feature Flags 完整参考：allocator、TLS backend、互斥规则、运行时注意事项及构建组合。
---

# Feature Flags

## 源码位置

- `Cargo.toml`
- `src/lib.rs`（allocator 全局安装）
- `src/core/gateway/runtime/server/listener_builder.rs`（TLS listener 守卫）
- `src/core/gateway/tls/runtime/`（TLS 运行时守卫）
- `src/bin/edgion_gateway.rs`、`src/bin/edgion_controller.rs`、`src/bin/edgion_ctl.rs`（rustls crypto provider 初始化）

## 默认构建

```bash
cargo build    # 等价于 --features allocator-jemalloc,boringssl
```

## Feature 矩阵

| Feature | 互斥组 | 默认 | 编译时效果 | 运行时含义 |
|---------|--------|------|-----------|-----------|
| `allocator-jemalloc` | Allocator | 是 | 引入 `tikv-jemallocator` | 非 MSVC 目标上设为全局分配器 |
| `allocator-mimalloc` | Allocator | 否 | 引入 `mimalloc` | 设为全局分配器 |
| `allocator-system` | Allocator | 否 | 无额外依赖 | 使用系统分配器 |
| `boringssl` | TLS Backend | 是 | 启用 Pingora BoringSSL + `boring-sys` | 解锁 Gateway TLS 完整运行时路径 |
| `openssl` | TLS Backend | 否 | 启用 Pingora OpenSSL | 同样解锁 Gateway TLS 完整运行时路径 |
| `rustls` | TLS Backend | 否 | 启用 Pingora rustls 后端 | **不完全对等**，见下方说明 |
| `legacy_route_tests` | 独立 | 否 | 仅声明 | 保留/占位，当前无实际 `cfg` 调用点 |

## 互斥规则

- **Allocator 组**：`jemalloc` / `mimalloc` / `system`，三选一。不要在同一次构建中叠加多个，除非你同时修改了 `src/lib.rs` 中的全局分配器接线。
- **TLS Backend 组**：`boringssl` / `openssl` / `rustls`，三选一。
- 跨组可自由组合。

## rustls 注意事项

Gateway TLS 运行时代码在多处使用 `#[cfg(any(feature = "boringssl", feature = "openssl"))]` 守卫：

- `src/core/gateway/runtime/server/listener_builder.rs`
- `src/core/gateway/tls/runtime/gateway/mod.rs`
- `src/core/gateway/tls/runtime/backend/mod.rs`

实际影响：

- `rustls` 仍用于 Controller / CLI / 客户端侧的 TLS 栈
- `rustls` 构建 ≠ "所有 Gateway TLS listener 和 backend 运行时功能可用"
- 如果任务涉及 HTTPS listener、TLSRoute 代理、数据面 TLS 运行时，优先使用 `boringssl` 或 `openssl`

## Allocator 注意事项

`src/lib.rs` 中 jemalloc 全局安装守卫为：

```rust
#[cfg(all(feature = "allocator-jemalloc", not(target_env = "msvc")))]
```

Windows/MSVC 环境下即使指定了 `allocator-jemalloc` feature，实际也不会安装。

## Binary 启动说明

三个 bin 在启动时均会安装 rustls crypto provider：

- `src/bin/edgion_gateway.rs`
- `src/bin/edgion_controller.rs`
- `src/bin/edgion_ctl.rs`

这是为了支持 repo 中依赖 rustls 的库，**不会**覆盖 Cargo feature 矩阵对 Gateway 数据面 TLS 后端的选择。

## 构建示例

| 场景 | 命令 |
|------|------|
| 默认开发构建 | `cargo build` |
| 默认 release | `cargo build --release` |
| mimalloc + OpenSSL | `cargo build --release --no-default-features --features "allocator-mimalloc,openssl"` |
| 系统分配器 + BoringSSL | `cargo build --release --no-default-features --features "allocator-system,boringssl"` |
| rustls 实验 | `cargo build --no-default-features --features "allocator-jemalloc,rustls"` |
