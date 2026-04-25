// ScenarioCtx — 所有 scenario 共享的上下文与组合算子。

use crate::framework::TestContext;
use anyhow::Result;
use std::path::PathBuf;
use std::time::Duration;

/// 每个 scenario 执行时获得的上下文。
/// 持有：
/// - `test_ctx`：现有 suite framework 的 `TestContext`（用于 `assert_phase` 调用 suite）
/// - `conf_root`：`examples/test/conf/` 绝对路径
/// - `conf_path`：当前 scenario 的相对 conf 目录（e.g. `Gateway/DynamicTest`）
/// - `controller_url`：Controller Admin API 基址（用于 apply）
/// - `gateway_url`：Gateway 观察端点基址（用于 resource_diff）
/// - `project_root`：仓库根，用于 subprocess 调用 resource_diff
pub struct ScenarioCtx {
    pub test_ctx: TestContext,
    pub conf_root: PathBuf,
    pub conf_path: String,
    pub controller_url: String,
    pub gateway_url: String,
    pub project_root: PathBuf,
}

impl ScenarioCtx {
    /// 返回当前 scenario 完整 conf 目录的绝对路径。
    pub fn scenario_dir(&self) -> PathBuf {
        self.conf_root.join(&self.conf_path)
    }

    /// 相对 scenario 根目录解析子路径。
    pub fn resolve(&self, rel: &str) -> PathBuf {
        self.scenario_dir().join(rel)
    }

    /// 简单包装 tokio sleep。
    pub async fn sleep(&self, dur: Duration) {
        tokio::time::sleep(dur).await;
    }
}

// apply / delete / wait 等方法在后续 Task 中逐步加入。

impl ScenarioCtx {
    /// Apply 一个相对于 scenario conf 目录的子路径（文件或目录）。
    pub async fn apply_dir(&self, rel_path: &str) -> Result<()> {
        let path = self.resolve(rel_path);
        if !path.exists() {
            anyhow::bail!("apply target does not exist: {}", path.display());
        }
        let client = crate::dynamic::apply::build_client(&self.controller_url)?;
        crate::dynamic::apply::apply_path(&client, &path).await
    }

    /// 按 `delete/resources_to_delete.txt` 列表删除资源。
    pub async fn delete_resources(&self, list_rel_path: &str) -> Result<()> {
        let path = self.resolve(list_rel_path);
        if !path.exists() {
            // 列表文件不存在视为"无需删除"，返回 Ok（与旧 bash 行为一致）。
            return Ok(());
        }
        let client = crate::dynamic::apply::build_client(&self.controller_url)?;
        crate::dynamic::apply::delete_list(&client, &path).await
    }
}

impl ScenarioCtx {
    /// 等待 controller 与 gateway 资源状态对齐。
    /// 超时默认 5s、轮询间隔 500ms —— 对热更新场景足够且显著优于裸 sleep 3s。
    pub async fn wait_resource_sync(&self) -> Result<()> {
        crate::dynamic::wait::wait_for_resource_sync(
            &self.project_root,
            &self.controller_url,
            &self.gateway_url,
            Duration::from_secs(5),
            Duration::from_millis(500),
        )
        .await
    }
}

impl ScenarioCtx {
    /// 根据当前 scenario + phase 名派发到对应 TestSuite 并运行。
    /// 所有 test case 必须全部 pass，否则返回 Err。
    pub async fn assert_phase(&self, scenario: &crate::dynamic::scenario::Scenario, phase: &str) -> Result<()> {
        use crate::framework::TestRunner;

        let ps = scenario
            .phase_suite(phase)
            .ok_or_else(|| anyhow::anyhow!("Scenario {} has no phase '{}'", scenario.id, phase))?;
        let suite = (ps.factory)();

        let mut runner = TestRunner::new(self.test_ctx.clone());
        runner.add_suite(suite);
        let results = runner.run().await;
        if !results.has_failures() {
            Ok(())
        } else {
            let mut failed_names = Vec::new();
            for sr in &results.suite_results {
                for (name, tr) in &sr.test_results {
                    if !tr.passed {
                        failed_names.push(format!(
                            "{}::{} ({})",
                            sr.name,
                            name,
                            tr.error.clone().unwrap_or_else(|| "no error".into())
                        ));
                    }
                }
            }
            Err(anyhow::anyhow!(
                "Scenario {} phase '{}' had {}/{} failing test(s): {:?}",
                scenario.id,
                phase,
                results.total_failed(),
                results.total_tests(),
                failed_names
            ))
        }
    }
}
