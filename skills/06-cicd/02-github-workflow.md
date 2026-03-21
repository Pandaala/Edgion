---
name: github-workflow
description: Use when changing GitHub Actions CI, release tags, Docker image publishing, or the shared setup-rust local action used by Edgion workflows.
---
# GitHub Workflow

用于回答这些问题：

- GitHub CI 现在到底跑了哪些 job、命令和缓存
- tag 发布后 Docker 镜像是怎么分架构构建并合并 manifest 的
- `setup-rust` 本地 action 该怎么改，才不会让 `ci.yml` 和 `build-image.yml` 一起坏掉
- 本地要如何尽量复现 CI / release 构建路径

## 先读这些真实入口

- [../../.github/workflows/ci.yml](../../.github/workflows/ci.yml)
- [../../.github/workflows/build-image.yml](../../.github/workflows/build-image.yml)
- [../../.github/actions/setup-rust/action.yml](../../.github/actions/setup-rust/action.yml)
- [../../docker/Dockerfile.runtime](../../docker/Dockerfile.runtime)
- [00-local-build.md](00-local-build.md)
- [01-docker-build.md](01-docker-build.md)

## 当前 CI 流水线

`ci.yml` 在 `push` / `pull_request` 到 `main` 或 `master` 时触发，顺序是：

1. `check` -> `cargo check --all-targets`
2. `fmt` -> `cargo fmt --all -- --check`
3. `agent-docs` -> `make check-agent-docs`
4. `clippy` -> `cargo clippy --all-targets`
5. `test` -> `cargo test --all`
6. `ci-success` -> 汇总前五个 job 的结果，给 branch protection 一个稳定检查点

几个关键事实：

- `check`、`fmt`、`clippy`、`test` 四个 Rust job 都会先走本地 action [../../.github/actions/setup-rust/action.yml](../../.github/actions/setup-rust/action.yml)
- `agent-docs` 只需要 checkout + `make check-agent-docs`，不依赖 Rust toolchain 初始化
- `fmt` 不安装系统依赖，也不开 cargo cache；其他构建型 Rust job 会装依赖并开启 cache
- 当前 CI 命令没有 `--all-features`，改命令前要先确认是否真的要扩大矩阵，而不是顺手把 CI 成本抬高

## `setup-rust` 本地 action 负责什么

这个 action 是 CI 和 release 共享的公共前置层，当前职责只有三件事：

1. 按需安装 Ubuntu 构建依赖：
   - `build-essential`
   - `cmake`
   - `pkg-config`
   - `protobuf-compiler`
   - `libssl-dev`
   - `libclang-dev`
   - `clang`
2. 恢复 / 保存 Cargo cache
3. 安装 rustup toolchain、可选 component、可选 target

如果你改它，要把它当成“共享基础设施”而不是某个 job 的私有脚本：

- 改缓存路径或 key 时，要同时考虑 `ci.yml` 和 `build-image.yml`
- 改依赖列表时，要核对 [../../build.rs](../../build.rs) 和 Docker builder 依赖是否仍然一致
- 改 toolchain/component/target 输入时，要保证 `fmt`、`clippy`、交叉编译都还能表达

## 当前 release 工作流

`build-image.yml` 在 push tag `v*` 时触发，分成 4 段：

1. `prepare-cargo-cache`
   - checkout
   - 调用 `setup-rust`
   - 只缓存 `~/.cargo/registry` 和 `~/.cargo/git`
   - `cargo fetch`

2. `build-binaries`
   - `amd64` / `arm64` 矩阵
   - arm64 额外安装 `gcc-aarch64-linux-gnu` / `g++-aarch64-linux-gnu`
   - 构建三个二进制：
     - `edgion-gateway`
     - `edgion-controller`
     - `edgion-ctl`
   - 上传产物 `edgion-binaries-<arch>`

3. `build-and-push-images`
   - 下载上一阶段的二进制产物
   - 用 [../../docker/Dockerfile.runtime](../../docker/Dockerfile.runtime) 打 runtime 镜像
   - 逐架构 push：
     - `pandaala/edgion-gateway:<tag>_amd64`
     - `pandaala/edgion-gateway:<tag>_arm64`
     - `pandaala/edgion-controller:<tag>_amd64`
     - `pandaala/edgion-controller:<tag>_arm64`

4. `merge-manifests`
   - 从 `refs/tags/vX.Y.Z` 提取：
     - 完整版本 `X.Y.Z`
     - minor `X.Y`
     - major `X`
   - 合并 manifest，最终生成：
     - `:<full>`
     - `:<minor>`
     - `:<major>`
     - `:latest`

## 本地复现命令

### 复现 CI 检查

```bash
cargo check --all-targets
cargo fmt --all -- --check
make check-agent-docs
cargo clippy --all-targets
cargo test --all
```

### 复现 release 二进制构建

```bash
# amd64
cargo build --release --bin edgion-gateway --bin edgion-controller --bin edgion-ctl \
  --target x86_64-unknown-linux-gnu

# arm64
rustup target add aarch64-unknown-linux-gnu
cargo build --release --bin edgion-gateway --bin edgion-controller --bin edgion-ctl \
  --target aarch64-unknown-linux-gnu
```

### 复现 runtime 镜像封装

```bash
docker build \
  --file docker/Dockerfile.runtime \
  --build-arg BINARY=gateway \
  --build-arg BINARY_PATH=target/x86_64-unknown-linux-gnu/release \
  -t edgion-gateway:local .
```

## 高风险改动点

- 改 `.github/workflows/*.yml` 时，先确认本地 action 的输入还够用
- 改 tag 命名规则时，连带检查 `merge-manifests` 的版本提取逻辑
- 改二进制名、产物路径或 target triple 时，连带检查 artifact upload/download 和 `Dockerfile.runtime`
- 改 `cache-key-prefix` 或缓存路径时，避免不同 job 互相污染或完全失去复用
- 改 CI 命令时，优先说明是“对齐本地常用命令”还是“扩大 CI 覆盖范围”

## 审查清单

- workflow 触发条件和分支 / tag 规则是否仍然正确
- 本地 action 路径是否是 `./.github/actions/setup-rust`
- `agent-docs` job 是否仍覆盖 `AGENTS.md` / `skills/` / `docs` 入口层
- `fmt` / `clippy` job 是否仍只装它们真正需要的东西
- release 是否仍只发布 `gateway` / `controller` 两个 runtime 镜像
- 版本 tag、镜像 tag、manifest tag 是否保持一致

## 相关

- [00-local-build.md](00-local-build.md)
- [01-docker-build.md](01-docker-build.md)
- [../02-development/06-feature-flags.md](../02-development/06-feature-flags.md)
