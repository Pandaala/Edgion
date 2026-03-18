---
name: config-reference
description: Configuration reference and selection guide for Edgion. Use when changing controller or gateway TOML, work_dir/path behavior, conf_center mode, log outputs, or GatewayClass-level runtime defaults via EdgionGatewayConfig.
---
# 配置参考

> 这个 skill 解决的不是“字段叫什么”，而是“这次配置应该改哪一层”。

## 先选配置层

### 1. Controller 进程级配置

适合改：
- Controller gRPC / Admin 监听
- `conf_center` 模式
- `conf_sync.no_sync_kinds`
- controller 日志和 work_dir

先读：
- [references/config-reference-controller.md](references/config-reference-controller.md)

### 2. Gateway 进程级配置

适合改：
- Gateway 到 Controller 的 gRPC 地址
- system log / access log / ssl log / tcp log / udp log
- Pingora worker、keepalive、error log
- RateLimit 全局配置

先读：
- [references/config-reference-gateway.md](references/config-reference-gateway.md)

### 3. GatewayClass / 运行时默认配置

适合改：
- `server` 线程和 keepalive
- `httpTimeout`
- `realIp`
- `securityProtect`
- 全局插件、preflight policy

先读：
- [references/config-reference-edgion-gateway-config.md](references/config-reference-edgion-gateway-config.md)

## 路径与优先级

### `work_dir` 优先级

Controller 和 Gateway 都遵循：

1. CLI `--work-dir`
2. 环境变量 `EDGION_WORK_DIR`
3. TOML `work_dir`
4. 默认 `.` 

### 你要特别记住的两个现实约束

1. `logging.log_dir` 只影响 system log。
   `access_log` / `ssl_log` / `tcp_log` / `udp_log` / `tls_log` 走的是各自的 `output.localFile.path`。

2. Controller 的 `conf_center.file_system.conf_dir` 当前实现并不会自动按 `work_dir` 重写。
   它在 FileSystemStorage 里是按进程当前工作目录解析的，所以这块最稳的做法是用绝对路径，或者明确知道当前 cwd。

## 当前实现里的重要坑位

- Gateway 配置结构里有 `gateway.admin_listen`，但当前启动逻辑仍然把 Admin API 固定绑在 `5900`
- Gateway Metrics API 当前固定绑在 `5901`
- CLI 真正的配置文件参数是 `--config-file`，不是旧注释里的 `--config`
- Controller 的 `conf_sync.no_sync_kinds` 一旦配置，会整体覆盖默认值，不是 append
- Gateway 的 `access_log` / `ssl_log` / `tcp_log` / `udp_log` / `tls_log` 主要靠 TOML 文件配置，不走 CLI 覆盖

## 仓库里的权威入口

- Controller TOML：`config/edgion-controller.toml`
- Gateway TOML：`config/edgion-gateway.toml`
- GatewayClass 运行时默认：`src/types/resources/edgion_gateway_config.rs`
- work_dir 行为：`src/types/work_dir.rs`

## 建议的排障顺序

如果你不确定“配置没生效”到底是哪一层出问题，按这个顺序：

1. 确认改的是正确层级
2. 确认 CLI 是否把文件值覆盖掉了
3. 确认相对路径是否真的按你以为的基准目录解析
4. 用 `edgion-ctl --target center/server/client` 看配置是不是已经进入下一层
5. 如仍异常，回到 [../testing/03-debugging.md](../testing/03-debugging.md)

## 相关资料

- [references/config-reference-controller.md](references/config-reference-controller.md)
- [references/config-reference-gateway.md](references/config-reference-gateway.md)
- [references/config-reference-edgion-gateway-config.md](references/config-reference-edgion-gateway-config.md)
- [../../docs/zh-CN/dev-guide/work-directory.md](../../docs/zh-CN/dev-guide/work-directory.md)
