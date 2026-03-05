---
name: docker-build
description: Docker multi-stage build with cargo-chef, multi-arch support.
---
# Docker 编译

> Docker 多阶段构建、cargo-chef 缓存优化、多架构（amd64/arm64）支持。
>
> **TODO (2026-02-25): P1**
> - [ ] Dockerfile 结构解析（`docker/Dockerfile`：chef prepare → chef cook → build → runtime）
> - [ ] cargo-chef 工作原理与缓存策略
> - [ ] 多架构构建配置（`--platform linux/amd64,linux/arm64`）
> - [ ] 构建参数（TLS 后端选择、Feature 传递）
> - [ ] 镜像大小优化实践
> - [ ] 本地 Docker 构建命令示例

## 快速参考

```bash
# 本地构建
docker build -f docker/Dockerfile -t edgion .

# 多架构构建（需要 buildx）
docker buildx build -f docker/Dockerfile \
  --platform linux/amd64,linux/arm64 \
  -t pandaala/edgion-gateway:latest \
  --push .
```

## 构建阶段

```
┌─────────────────────────────────────────┐
│ Stage 1: chef (cargo-chef prepare)      │
│   → 生成 recipe.json（依赖清单）         │
├─────────────────────────────────────────┤
│ Stage 2: cook (cargo-chef cook)         │
│   → 仅编译依赖（利用 Docker 层缓存）     │
├─────────────────────────────────────────┤
│ Stage 3: build                          │
│   → 编译项目代码                         │
├─────────────────────────────────────────┤
│ Stage 4: runtime                        │
│   → 最小运行时镜像（仅含二进制）          │
└─────────────────────────────────────────┘
```

## Key Files

- `docker/Dockerfile` — 多阶段构建定义
