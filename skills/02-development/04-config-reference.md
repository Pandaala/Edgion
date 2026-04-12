# 配置参考

用于这些任务：

- 查找 Controller、Gateway 或 `EdgionGatewayConfig` 的配置项定义
- 理解 TOML 配置文件的结构和各字段含义
- 添加新配置项时确认正确的 Schema 位置

## 配置文件概览

Edgion 有三类主要配置文件：

| 文件 | 对应组件 | 典型位置 |
|------|---------|---------|
| `controller.toml` | Controller | `{conf_dir}/controller.toml` |
| `gateway.toml` | Gateway | `{conf_dir}/gateway.toml` |
| `EdgionGatewayConfig` CRD | Gateway（K8s 驱动） | K8s 资源 |

## 快速查找配置项

1. **进入配置 SKILL 导航**：[../02-features/02-config/SKILL.md](../02-features/02-config/SKILL.md)
2. 根据配置项所属组件选择对应的 Schema 文档
3. 相对路径行为说明见 `docs/zh-CN/dev-guide/work-directory.md`

## 添加新配置项步骤

1. 确定配置项归属：Controller、Gateway 还是 CRD
2. 在对应的配置结构体中添加字段（通常在 `src/core/*/config/` 下）
3. 添加默认值和 serde 注解
4. 更新 `skills/02-features/02-config/` 下对应的 Schema 文档
5. 如有破坏性变更，同步更新示例配置文件

## 常见踩坑

- 相对路径在不同 `work_directory` 设置下行为不同，建议测试时使用绝对路径
- `[local_auth]` 段是可选的，缺省时自动生成随机凭据
- Gateway TOML 和 `EdgionGatewayConfig` CRD 控制不同维度，不要混淆

## 参考文档

- 配置 SKILL 导航：[../02-features/02-config/SKILL.md](../02-features/02-config/SKILL.md)
