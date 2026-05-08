# Edgion Unit Testing

用于这些任务：

- 为新增或修改的模块编写单元测试
- 提升代码覆盖率
- 验证单个函数 / 结构体 / 模块的行为正确性

## 设计原则

- 单元测试 + 集成测试组合目标：**99% 代码覆盖率**
- 单元测试负责：纯逻辑、数据结构、解析、校验、工具函数
- 集成测试负责：跨进程交互、配置同步、路由行为、插件运行时

## 项目约定

### 测试位置

Edgion 采用 Rust 标准的 **inline test module** 模式，测试写在源文件底部：

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_something() {
        // ...
    }
}
```

不使用独立的 `tests/` 目录做单元测试。

### 测试文件分布

单元测试主要覆盖以下区域：

| 区域 | 示例路径 | 测试重点 |
|------|---------|---------|
| 资源类型解析 | `src/types/resources/*.rs` | CRD 结构反序列化、字段校验、preparse |
| 插件配置 | `src/types/resources/edgion_plugins/plugin_configs/*.rs` | `validate_and_init()`、默认值、边界条件 |
| LinkSys 配置 | `src/types/resources/link_sys/*.rs` | 连接器配置解析与校验 |
| Core 工具函数 | `src/core/common/utils/mod.rs` | 字符串处理、路径工具、通用逻辑 |
| 配置系统 | `src/core/common/config/mod.rs` | TOML 解析、配置合并、默认值 |
| Gateway 后端 | `src/core/gateway/backends/mod.rs` | 后端选择、负载均衡逻辑 |
| 工作目录 | `src/types/work_dir.rs` | 路径解析、目录创建、权限校验 |

## 运行方式

```bash
# 全量单元测试
cargo test --all

# 只跑某个模块的测试
cargo test --lib -p edgion -- types::resources::http_route

# 只跑某个具体测试函数
cargo test --lib test_function_name

# 显示测试输出（调试用）
cargo test --all -- --nocapture

# 顺序执行（避免并发问题）
cargo test --all -- --test-threads=1
```

## 编写规范

### 1. 每个新增 / 修改的公开函数都应有对应测试

```rust
pub fn parse_duration(s: &str) -> Result<Duration, Error> { ... }

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_duration_seconds() {
        assert_eq!(parse_duration("30s").unwrap(), Duration::from_secs(30));
    }

    #[test]
    fn test_parse_duration_invalid() {
        assert!(parse_duration("abc").is_err());
    }
}
```

### 2. 用 `tempfile` 处理文件系统测试

```rust
#[test]
fn test_work_dir_creation() {
    let tmp = tempfile::TempDir::new().unwrap();
    let work_dir = WorkDir::new(tmp.path().to_path_buf());
    assert!(work_dir.ensure_dirs().is_ok());
}
```

### 3. 测试命名清晰表达意图

```rust
#[test]
fn test_cors_config_default_allows_all_origins() { ... }

#[test]
fn test_cors_config_rejects_empty_allow_methods() { ... }

#[test]
fn test_reference_grant_matches_exact_namespace() { ... }
```

### 4. 测试边界条件和错误路径

每个 `validate_and_init()` 至少覆盖：
- 合法默认配置
- 必填字段缺失
- 非法值（空字符串、超范围数字、格式错误）
- 边界值

### 5. 避免测试中的反模式

- **不要** mock 数据库或外部服务 — 这属于集成测试范畴
- **不要** 在单元测试中启动 HTTP server 或 gRPC — 用集成测试
- **不要** 依赖文件系统的绝对路径 — 用 `tempfile`
- **不要** 写只验证 "不 panic" 的空测试

## 删除 / 合并单测前必须读：判定规则

精简 / 合并单测时（合并参数化、删除被集成等价覆盖的测试），**符合以下任一条的必须保留为单元测试**，不得删除、不得合并到失去粒度：

1. **数据结构不变量** — LRU 计数器下溢、`hit_ratio` 分母为 0 的 NaN 防护
2. **错误信息文本** — Condition 的 `reason` / `message` 到枚举的映射（集成测无法精确比对文本）
3. **算法正确性** — IP CIDR 合法性、radix 树构建、hostname 通配符覆盖关系
4. **spec 一致性** — Gateway API spec 要求的排序/过滤行为（如 regex 长度优先）
5. **wire format / 序列化兼容** — serde `rename_all` 等属性（CLI / 跨进程硬解析的字段）
6. **安全边界** — mTLS `verify_depth` 范围、cipher 白名单、SAN 覆盖逻辑
7. **版本兼容** — v1alpha3→v1 等 CRD 版本转换（集成测通常只跑最新版本）
8. **回归修复** — 有历史 bug 编号（如 P0-07 leastconn 计数）的测试
9. **纯密码学 / JWT 签发校验** — 不能靠黑盒请求证明

### 关键不可动清单（举例，非全量）

| 位置 | 原因 |
|---|---|
| `src/core/gateway/lb/runtime_state/mod.rs` leastconn 计数 | P0-07 回归 |
| `src/core/gateway/tls/validation/cert.rs` | mTLS verify_depth / SAN 通配符 |
| `src/core/gateway/cache/lru.rs` 不变量测试 | LRU 下溢 / hit_ratio |
| `src/core/gateway/routes/grpc/match_engine.rs` 排序测试 | spec conformance |
| `src/core/gateway/runtime/matching/route.rs` allowed_routes | spec conformance |
| `src/core/controller/conf_mgr/sync_runtime/resource_processor/status_utils.rs` Condition 映射 | reason/message 文本 |
| `src/core/controller/conf_mgr/sync_runtime/resource_processor/listener_port_manager.rs` 端口冲突矩阵 | 协议混合规则 |
| `src/core/controller/conf_mgr/conf_center/file_system/resource_controller.rs` v1alpha3→v1 | 版本兼容 |
| `src/core/common/matcher/**` | IP / host 匹配算法 |
| `src/core/common/utils/duration.rs` / `real_ip_extractor.rs` | 时间表达式 / 头优先级算法 |
| `src/core/common/unified_auth/mod.rs` `from_configs` / `require_auth` | 多源认证装配 |
| `src/core/common/local_auth/handlers.rs` `AuthStatus` JSON wire 格式 | CLI 硬解析依赖 |

### Reviewer checklist（精简 PR 必看）

PR 删除上表中任何测试，必须在描述里说明：
- **原因** — 为什么觉得可以删
- **替代覆盖** — 集成测哪个 suite / 哪条 case 等价覆盖了
- **为什么集成能精确等价** — 集成测对错误文本、内部不变量、异常路径枚举系统性盲，必须证明这次不踩到

## CI 集成

单元测试在 CI 中通过 `.github/workflows/ci.yml` 自动运行：

```yaml
- name: Run tests
  run: cargo test --all
```

每次 push 到 `main`/`master` 和每个 PR 都会触发。

## 与集成测试的分工

| 维度 | 单元测试 | 集成测试 |
|------|---------|---------|
| **范围** | 单个函数 / 模块 | Controller + Gateway + test_server 跨进程 |
| **运行速度** | 秒级 | 分钟级 |
| **外部依赖** | 无（最多 tempfile） | 需要启动多个进程 |
| **覆盖重点** | 解析、校验、纯逻辑 | 配置同步、路由、插件运行时、TLS |
| **运行命令** | `cargo test --all` | `run_integration.sh` |
| **失败含义** | 某个函数逻辑有 bug | 跨组件交互有问题 |
