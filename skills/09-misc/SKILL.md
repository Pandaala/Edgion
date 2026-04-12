---
name: 09-misc
description: Miscellaneous skill for Edgion. Includes CI/CD (compiling binaries, feature flags, Docker builds, release workflows) and TLS debugging.
---

# 09 杂项

> Edgion 构建与发布流程。Rust 1.75+ 单 Crate 项目，默认 features: `allocator-jemalloc` + `boringssl`。
> 三个二进制：`edgion-gateway`（数据面）、`edgion-controller`（控制面）、`edgion-cli`（CLI）。

## 文件清单

| 文件 | 主题 | 状态 |
|------|------|------|
| [cicd/](cicd/) | CI/CD：本地构建、Docker 构建、GitHub Workflow、Release 发布 | ✅ |
| [debugging-tls-gateway.md](debugging-tls-gateway.md) | K8s 环境下 TLS 专项调试（kubectl exec + 日志文件 + grep 模式） | ✅ |
| [debugging-conformance.md](debugging-conformance.md) | Conformance 测试调试：rv/sv/access log 三元组、orbstack-dev.sh 日志命令、各 profile 失败模式 | ✅ |

## 快速参考

### 本地构建
```bash
# 开发模式（快速编译）
cargo build

# 发布模式
cargo build --release

# 指定 TLS 后端（rustls 实验构建）
cargo build --release --no-default-features --features "allocator-jemalloc,rustls"

# 代码检查
cargo check && cargo clippy && cargo fmt --check
make check-agent-docs
```

补充入口：

- 本地构建细节见 [cicd/00-local-build.md](cicd/00-local-build.md)
- Feature 组合细节见 [../02-features/01-binary-and-deployment/02-feature-flags.md](../02-features/01-binary-and-deployment/02-feature-flags.md)

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
| `rustls` | 使用 rustls 作为 TLS 后端 | |
| `allocator-mimalloc` | 使用 mimalloc 内存分配器 | |
| `allocator-system` | 使用系统默认分配器 | |
| `legacy_route_tests` | 启用旧版路由测试 | |

> ⚠️ `boringssl`、`rustls` 二选一，不可同时启用。
