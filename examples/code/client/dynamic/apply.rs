// Apply / delete helpers — 直接复用 ctl-cli 的内部实现，保持与 edgion-cli 一致。
//
// API 参考（已核对 src/core/ctl/cli/{client,commands}.rs）：
//   EdgionClient::new(target: TargetType, server: Option<String>, socket: Option<PathBuf>,
//                     controller_config: Option<String>) -> Result<Self>
//   apply(client, file_path: &str, dry_run: bool) -> Result<()>
//   delete(client, kind: Option<&str>, name: Option<&str>, namespace: Option<&str>,
//          file: Option<&str>) -> Result<()>
//
// admin token 会由 EdgionClient::new 内部自动从 ~/.edgion/token 读取（见 client.rs）。

use anyhow::{Context, Result};
use std::path::Path;

use edgion::core::ctl::cli::client::EdgionClient;
use edgion::core::ctl::cli::commands::apply::apply;
use edgion::core::ctl::cli::commands::delete::delete;
use edgion::core::ctl::cli::TargetType;

/// 构造一个面向 Controller (ConfCenter) 的 EdgionClient。
pub fn build_client(controller_url: &str) -> Result<EdgionClient> {
    EdgionClient::new(TargetType::Center, Some(controller_url.to_string()), None, None)
        .context("Failed to build EdgionClient for scenario")
}

/// Apply 一个文件或目录（目录会遍历 *.yaml / *.yml）。
///
/// **NOTE:** 当 `path` 是目录时，`apply_directory` 内部对单文件失败仅 print_error
/// 后继续，整体返回 `Ok(())`（见 `src/core/ctl/cli/commands/apply.rs:38`）。
/// 因此目录模式下个别 YAML 失败不会传播为错误；scenario 应在 apply 后通过
/// `wait_resource_sync` 或 phase 断言显式校验 gateway 状态。
pub async fn apply_path(client: &EdgionClient, path: &Path) -> Result<()> {
    let path_str = path
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("Non-UTF8 path: {:?}", path))?;
    apply(client, path_str, false)
        .await
        .with_context(|| format!("apply failed for {}", path.display()))
}

/// 按列表文件删除资源。每行格式：`Kind/Namespace/Name`，`#` 开头的行与空行忽略。
/// Namespace 字段必须非空；cluster-scoped 资源暂不支持（如有需要，扩展该格式）。
pub async fn delete_list(client: &EdgionClient, list_file: &Path) -> Result<()> {
    let content = tokio::fs::read_to_string(list_file)
        .await
        .with_context(|| format!("Failed to read delete list: {}", list_file.display()))?;
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let parts: Vec<&str> = line.split('/').collect();
        if parts.len() != 3 {
            anyhow::bail!("Invalid delete list line (expected Kind/Namespace/Name): {}", line);
        }
        let (kind, namespace, name) = (parts[0], parts[1], parts[2]);
        delete(client, Some(kind), Some(name), Some(namespace), None)
            .await
            .with_context(|| format!("Failed to delete {}/{}/{}", kind, namespace, name))?;
    }
    Ok(())
}
