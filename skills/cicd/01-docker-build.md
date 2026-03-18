---
name: docker-build
description: Use when building Edgion images with Docker, understanding the multi-stage build, cargo-chef caching, feature passing, or runtime-image layout.
---
# Docker 编译

用于回答这些问题：

- `docker/Dockerfile` 的多阶段构建到底怎么走
- `cargo-chef` 缓存的是哪一层
- 构建参数 `BINARY`、`BUILD_TYPE`、`FEATURES` 分别影响什么
- 什么时候用 `docker/Dockerfile`，什么时候用 `docker/Dockerfile.runtime`

## 主要文件

- `docker/Dockerfile`
- `docker/Dockerfile.builder`
- `docker/Dockerfile.runtime`

## `docker/Dockerfile` 的 4 个阶段

### 1. `chef`

- 基础镜像：`rust:<version>-slim`
- 安装 `cargo-chef`
- 只负责提供 `cargo chef` 命令

### 2. `planner`

- 复制 `Cargo.toml`、`Cargo.lock`、`src/lib.rs`、`src/bin`
- 运行 `cargo chef prepare --recipe-path recipe.json`
- 生成依赖配方，不编译业务代码

### 3. `builder`

- 安装构建依赖：
  - `g++`
  - `cmake`
  - `git`
  - `libclang-dev`
  - `perl`
  - `pkg-config`
  - `protobuf-compiler`
- 先执行 `cargo chef cook --release --features "${FEATURES}" --recipe-path recipe.json`
- 再复制完整源码并真正 `cargo build`

缓存意义：

- 依赖没变时，`cargo chef cook` 这一层能复用
- 这样业务代码改动不会每次都把整个依赖树重编一遍

### 4. `runtime`

- 运行时镜像：`debian:trixie-slim`
- 只安装 `ca-certificates` 和 `libssl3`
- 创建非 root 用户 `edgion`
- 拷贝最终二进制和默认配置

## 构建参数

| 参数 | 默认值 | 作用 |
|------|--------|------|
| `RUST_VERSION` | `1.92` | Rust toolchain 版本 |
| `BINARY` | `edgion-gateway` | 要构建的 bin 或 example 名称 |
| `BUILD_TYPE` | `bin` | `bin` 或 `example` |
| `FEATURES` | `default` | 传给 `cargo build` / `cargo chef cook` 的 Cargo features |

## 常用命令

### 构建 bin 镜像

```bash
docker build \
  --build-arg BINARY=edgion-gateway \
  -t edgion/edgion-gateway:local \
  -f docker/Dockerfile .
```

### 构建 controller

```bash
docker build \
  --build-arg BINARY=edgion-controller \
  -t edgion/edgion-controller:local \
  -f docker/Dockerfile .
```

### 构建 example

```bash
docker build \
  --build-arg BUILD_TYPE=example \
  --build-arg BINARY=test_server \
  -t edgion/edgion-test-server:local \
  -f docker/Dockerfile .
```

### 切换 features

```bash
docker build \
  --build-arg BINARY=edgion-gateway \
  --build-arg FEATURES="allocator-jemalloc,openssl" \
  -t edgion/edgion-gateway:openssl \
  -f docker/Dockerfile .
```

## `docker/Dockerfile.runtime` 什么时候用

这个文件不是源码编译镜像，而是“把已经编好的产物打包成运行时镜像”。

适合：

- CI 已经提前交叉编译好了二进制
- 只想快速打 runtime image
- 本地已有 `target/release` 产物，不想再在 Docker 里重新编译

关键参数：

| 参数 | 作用 |
|------|------|
| `BINARY` | `gateway` / `controller` 或 example 名 |
| `BUILD_TYPE` | `bin` 或 `example` |
| `BINARY_PATH` | 预编译产物目录 |
| `RUNTIME_BASE` | 运行时基础镜像，默认 `ubuntu:24.04` |
| `EXTRA_PACKAGES` | 额外运行时 apt 包 |

示例：

```bash
docker build \
  -f docker/Dockerfile.runtime \
  --build-arg BINARY=gateway \
  --build-arg BUILD_TYPE=bin \
  --build-arg BINARY_PATH=target/release \
  -t edgion/edgion-gateway:runtime-local .
```

## 多架构与 builder 镜像

`docker/Dockerfile.builder` 是单独的“构建环境镜像”，更适合：

- 从任意宿主机编 Linux 目标
- 多架构编译实验
- 不想把完整源码构建过程塞进最终业务镜像

示例思路：

```bash
docker build --platform linux/arm64 -t edgion-builder -f docker/Dockerfile.builder .
docker run --rm --platform linux/arm64 -v "$(pwd)":/project edgion-builder \
  cargo build --release --target aarch64-unknown-linux-gnu
```

## 最常见的误区

| 误区 | 更准确的说法 |
|------|--------------|
| `cargo-chef` 会缓存所有源码编译 | 它主要缓存“依赖层”，源码改动后的最终 build 仍会执行 |
| `FEATURES` 只影响最终 `cargo build` | 它同样影响 `cargo chef cook` 依赖缓存层 |
| `Dockerfile.runtime` 可以直接从源码编译 | 不能，它依赖预编译产物目录 |
| example 构建也会自动带 `edgion-ctl` | `BUILD_TYPE=example` 时不会打包 `edgion-ctl` |

## 相关

- [00-local-build.md](00-local-build.md)
- [../development/06-feature-flags.md](../development/06-feature-flags.md)
- `.github/workflows/build-image.yml`
