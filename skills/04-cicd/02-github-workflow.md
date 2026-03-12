---
name: github-workflow
description: GitHub Actions CI pipeline, release workflow, Docker Hub publishing.
---
# GitHub Workflow

> CI 流水线、Release 发布流程、Docker Hub 镜像推送。
>
> **TODO (2026-02-25): P1**
> - [ ] CI 流水线详解（`.github/workflows/ci.yml`：check → fmt → clippy → test）
> - [ ] Rust 工具链版本管理
> - [ ] Release 流程（git tag `v*` → `build-image.yml` 触发 → Docker Hub 推送）
> - [ ] Docker Hub 镜像命名规范（`pandaala/edgion-gateway`、`pandaala/edgion-controller`）
> - [ ] Branch 保护与 PR 检查策略
> - [ ] 缓存策略（Rust 编译缓存、Docker 层缓存）

## 发布流程

```
开发者操作:
  git tag v0.x.x
  git push origin v0.x.x

GitHub Actions 自动触发:
  build-image.yml
    → 多架构构建 (amd64 + arm64)
    → 推送到 Docker Hub: pandaala/edgion-*
```

## Key Files

- `.github/workflows/ci.yml` — CI 流水线
- `.github/workflows/build-image.yml` — Release 构建与推送
- `docker/Dockerfile` — 构建镜像定义
