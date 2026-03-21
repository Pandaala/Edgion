---
name: cicd
description: Build and CI/CD skill for Edgion. Use when compiling binaries, changing feature flags, updating Docker builds, or modifying CI and release workflows.
---

# 04 CI/CD 与构建

> Edgion 构建与发布流程。Rust 1.75+ 单 Crate 项目，默认 features: `allocator-jemalloc` + `boringssl`。
> 三个二进制：`edgion-gateway`（数据面）、`edgion-controller`（控制面）、`edgion-ctl`（CLI）。

## 文件清单

| 文件 | 主题 | 状态 |
|------|------|------|
| [00-local-build.md](00-local-build.md) | 本地编译命令、产物位置、Feature 组合、常见编译问题 | ✅ 已重构 |
| [01-docker-build.md](01-docker-build.md) | Docker 多阶段构建、cargo-chef、多架构支持 | ✅ 已重构 |
| [02-github-workflow.md](02-github-workflow.md) | CI 流水线、Release 发布、Docker Hub 推送 | ✅ 已重构 |

## 快速参考

### 本地构建
```bash
# 开发模式（快速编译）
cargo build

# 发布模式
cargo build --release

# 指定 TLS 后端
cargo build --release --no-default-features --features "allocator-jemalloc,openssl"

# 代码检查
cargo check && cargo clippy && cargo fmt --check
make check-agent-docs
```

补充入口：

- 本地构建细节见 [00-local-build.md](00-local-build.md)
- Feature 组合细节见 [../02-development/06-feature-flags.md](../02-development/06-feature-flags.md)

### Docker 构建
```bash
# 参考 docker/ 目录下的 Dockerfile
docker build -f docker/Dockerfile -t edgion .
```

### 发布流程
```
git tag v0.x.x → push → build-image.yml → Docker Hub pandaala/edgion-*
```

## Cargo Features 速查

| Feature | 说明 | 默认 |
|---------|------|------|
| `allocator-jemalloc` | 使用 jemalloc 内存分配器 | ✅ |
| `boringssl` | 使用 BoringSSL 作为 TLS 后端 | ✅ |
| `openssl` | 使用 OpenSSL 作为 TLS 后端 | |
| `rustls` | 使用 rustls 作为 TLS 后端 | |
| `allocator-mimalloc` | 使用 mimalloc 内存分配器 | |
| `allocator-system` | 使用系统默认分配器 | |
| `legacy_route_tests` | 启用旧版路由测试 | |

> ⚠️ `boringssl`、`openssl`、`rustls` 三选一，不可同时启用。
