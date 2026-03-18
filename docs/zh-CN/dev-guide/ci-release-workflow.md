# CI 与 Release 工作流指南

本文档面向维护 GitHub Actions、发布 Docker 镜像、或需要在本地复现 CI 行为的贡献者，说明当前 Edgion 的 CI / release 结构、共享 action、命令边界和常见改动风险。

> 面向 AI / Agent 的主 workflow 入口现在是 [../../../skills/cicd/02-github-workflow.md](../../../skills/cicd/02-github-workflow.md)。
> 本文档保留给人看的背景说明、发布流程解读和人工审查清单。

## 当前有哪两条主线

仓库里的 GitHub Actions 目前主要分成两条主线：

### 1. CI 检查

文件：

- `.github/workflows/ci.yml`

触发条件：

- push 到 `main` / `master`
- pull request 指向 `main` / `master`

职责：

- 基础编译检查
- 格式检查
- agent 文档 / skills / dev-guide 入口校验
- clippy
- 单元 / 集成测试入口中的 `cargo test --all`

### 2. Release 与镜像发布

文件：

- `.github/workflows/build-image.yml`

触发条件：

- push tag `v*`

职责：

- 预取 Cargo 依赖
- 构建 `amd64` / `arm64` 两套 Linux 二进制
- 生成并推送 runtime 镜像
- 合并 multi-arch manifest

## 共享基础设施：`setup-rust`

CI 和 release 现在都依赖同一个本地 action：

- `.github/actions/setup-rust/action.yml`

这个 action 负责三件事：

1. 安装 Rust toolchain、component、target
2. 按需安装 Ubuntu 构建依赖
3. 统一处理 Cargo cache

当前这层的目的很明确：

- 不让 `ci.yml` 和 `build-image.yml` 各自维护一套散落的 Rust 初始化步骤
- 修改依赖、cache、toolchain 时只改一个地方

因此，如果你改这个 action，不要把它当成某个 job 的私有脚本；它是当前仓库 CI / release 的共享基础设施。

## CI 现在实际跑什么

当前 `ci.yml` 的主检查顺序是：

1. `cargo check --all-targets`
2. `cargo fmt --all -- --check`
3. `make check-agent-docs`
4. `cargo clippy --all-targets`
5. `cargo test --all`
6. 汇总 job `ci-success`

这里有几个容易被误解的点：

- 当前 CI 命令没有默认扩大到 `--all-features`
- `make check-agent-docs` 专门负责兜住 `AGENTS.md`、`skills/` 和 dev-guide 入口层的一致性
- `fmt` job 不安装系统依赖，也不开 Cargo cache
- `ci-success` 的作用是给 branch protection 一个稳定汇总结果，不是重复跑检查

## Release 现在怎么走

当前 `build-image.yml` 大致分成四段：

### 1. `prepare-cargo-cache`

作用：

- checkout
- 调用 `setup-rust`
- cache `~/.cargo/registry` 与 `~/.cargo/git`
- `cargo fetch`

### 2. `build-binaries`

作用：

- 用矩阵分别构建 `amd64` 与 `arm64`
- 产出三个二进制：
  - `edgion-gateway`
  - `edgion-controller`
  - `edgion-ctl`
- arm64 额外安装交叉编译器

### 3. `build-and-push-images`

作用：

- 下载前一步的二进制产物
- 用 `docker/Dockerfile.runtime` 打 runtime 镜像
- 按架构推送镜像 tag

### 4. `merge-manifests`

作用：

- 从 `vX.Y.Z` tag 中提取版本号
- 合并 `amd64` / `arm64` 镜像
- 生成：
  - 完整版本 tag
  - minor tag
  - major tag
  - `latest`

## 本地命令和 workflow 的关系

对于这个仓库，命令归属建议保持分层：

- 仓库最常用命令放在 `AGENTS.md`
- workflow 级细节放在 skill
- 给人看的解释保留在 `docs/`

如果你只是想本地复现当前 CI，优先跑：

```bash
cargo check --all-targets
cargo fmt --all -- --check
make check-agent-docs
cargo clippy --all-targets
cargo test --all
```

如果你想接近 release 二进制构建，优先看：

- `skills/cicd/00-local-build.md`
- `skills/cicd/01-docker-build.md`
- `skills/cicd/02-github-workflow.md`

## 改 workflow 时最容易踩坑的点

### 1. 改了 workflow，没有同步共享 action

比如：

- 新增 target / component，但 `setup-rust` 不支持
- 改 cache 路径，但本地 action 还在写旧路径

### 2. 改了二进制构建路径，没有同步镜像打包

二进制产物路径、artifact upload/download、`Dockerfile.runtime` 是一条链，不能只改其中一段。

### 3. 改了 tag 规则，没有同步 manifest 合并逻辑

`merge-manifests` 当前默认 tag 是 `vX.Y.Z` 形式。  
如果发布规范改变，镜像 tag 推导逻辑也要一起看。

### 4. 把“本地常用命令”和“CI 必跑命令”混成一套

本地调试命令可以更灵活，但 CI 命令更强调稳定、成本和可重复性。  
不要因为某次本地排障方便，就顺手把 CI 扩成更重的矩阵。

## 人工审查清单

- trigger 条件是否仍然符合预期的分支 / tag 策略
- `.github/actions/setup-rust/action.yml` 是否还能覆盖所有 workflow 输入
- 缓存 key 是否会互相污染，或者完全失去命中
- Release 是否仍然只发布 `gateway` / `controller` runtime 镜像
- 镜像 tag、manifest tag、git tag 的语义是否一致
- 本地复现命令是否还能对齐 workflow 中真实执行的命令

## 如果要让 AI 帮忙改

直接从 skill 入口开始最稳：

- [../../../skills/cicd/02-github-workflow.md](../../../skills/cicd/02-github-workflow.md)

如果任务还涉及 feature 组合或构建依赖，再补看：

- [../../../skills/development/06-feature-flags.md](../../../skills/development/06-feature-flags.md)
- [../../../skills/cicd/00-local-build.md](../../../skills/cicd/00-local-build.md)
- [../../../skills/cicd/01-docker-build.md](../../../skills/cicd/01-docker-build.md)

## 相关文档

- [AI 协作与 Skills 使用指南](./ai-agent-collaboration.md)
- [知识来源映射与维护规则](./knowledge-source-map.md)
- [Makefile](../../../Makefile)
