---
name: local-build
description: Use when compiling Edgion locally, choosing a build mode, locating build artifacts, or debugging local build failures and feature combinations.
---
# 本地编译

用于回答这些问题：

- 现在本地应该跑 `cargo check`、`cargo build` 还是 `cargo build --release`
- 二进制和 examples 会产出到哪里
- 切换 allocator / TLS backend 时怎么编
- 本地编译为什么缺依赖、链接失败或交叉编译失败

## 先看这几个事实

- 默认 features 是 `allocator-jemalloc` + `boringssl`
- proto 编译来自仓库根的 `build.rs`，因此本地需要可用的 `protoc`
- Docker 构建环境已经显式安装了这些依赖，可作为本地依赖的可信参考：
  - `cmake`
  - `protobuf-compiler`
  - `pkg-config`
  - `libssl-dev`
  - `libclang-dev`
- CI 交叉编译 `aarch64-unknown-linux-gnu` 时会额外安装 `gcc-aarch64-linux-gnu` / `g++-aarch64-linux-gnu`

## 最常用命令

```bash
# 最快的语义检查
cargo check

# 默认开发构建
cargo build

# 发布构建
cargo build --release

# 常用二进制
cargo build --release --bin edgion-gateway --bin edgion-controller --bin edgion-ctl

# 常用 examples
cargo build --release --example test_server --example test_client
```

## Feature 组合

常用组合：

```bash
# 默认：jemalloc + boringssl
cargo build

# OpenSSL
cargo build --release --no-default-features --features "allocator-jemalloc,openssl"

# rustls 实验构建
cargo build --release --no-default-features --features "allocator-jemalloc,rustls"

# mimalloc + boringssl
cargo build --release --no-default-features --features "allocator-mimalloc,boringssl"

# system allocator + boringssl
cargo build --release --no-default-features --features "allocator-system,boringssl"
```

更详细的 feature 语义见：

- [../development/06-feature-flags.md](../development/06-feature-flags.md)

## 编译产物在哪

| 构建方式 | 产物位置 |
|---------|----------|
| `cargo build` | `target/debug/` |
| `cargo build --release` | `target/release/` |
| `cargo build --target <triple> --release` | `target/<triple>/release/` |
| `cargo build --example <name> --release` | `target/release/examples/` |

常见文件：

- `target/debug/edgion-gateway`
- `target/release/edgion-controller`
- `target/release/edgion-ctl`
- `target/release/examples/test_server`

## 交叉编译

CI 当前对 ARM64 Linux 的做法：

```bash
rustup target add aarch64-unknown-linux-gnu
cargo build --release --target aarch64-unknown-linux-gnu \
  --bin edgion-gateway --bin edgion-controller --bin edgion-ctl
```

如果是 Debian/Ubuntu 风格环境，通常还需要：

```bash
sudo apt-get install -y gcc-aarch64-linux-gnu g++-aarch64-linux-gnu
```

## 常见失败与优先排查

| 现象 | 先看什么 |
|------|----------|
| `protoc` / protobuf 相关错误 | 是否安装了 `protobuf-compiler`，以及 `build.rs` 会不会被执行 |
| BoringSSL 构建失败 | 是否安装了 `cmake`；当前 feature 是否启用了 `boringssl` |
| OpenSSL / `pkg-config` / `ssl` 链接失败 | 是否安装了 `pkg-config`、`libssl-dev` 或对应平台 OpenSSL 开发包 |
| ARM64 交叉编译失败 | 是否安装了 `aarch64` 交叉编译器，并设置好 target |
| 编出来了但 Gateway TLS 路径不可用 | 是否误用了 `rustls` 组合；当前 Gateway TLS runtime 仍主要依赖 `boringssl` / `openssl` |

## 需要一个干净、可复现的构建环境时

优先看：

- `docker/Dockerfile.builder`
- `docker/Dockerfile`
- `.github/workflows/build-image.yml`

这三处基本就是当前项目对“构建依赖最小集合”和“交叉编译做法”的真实落地。
