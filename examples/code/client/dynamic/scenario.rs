// Scenario 数据结构与 phase→suite 映射。

use crate::dynamic::ctx::ScenarioCtx;
use crate::framework::TestSuite;
use anyhow::Result;
use std::future::Future;
use std::pin::Pin;

/// scenario.run 的返回类型。每次调用产生一个独立的、可发送的 Future。
pub type ScenarioRunFuture = Pin<Box<dyn Future<Output = Result<()>> + Send>>;

/// scenario 执行入口的函数指针类型。
pub type ScenarioRunFn = fn(ScenarioCtx) -> ScenarioRunFuture;

/// Phase 名 → Suite 工厂。每个 scenario 自己声明它支持的 phase。
pub struct PhaseSuite {
    pub name: &'static str,
    pub factory: fn() -> Box<dyn TestSuite>,
}

/// 一个 dynamic scenario。
pub struct Scenario {
    /// 唯一标识（CLI 用），e.g. "BasicAuthDynamic"。
    pub id: &'static str,
    /// scenario 的 conf 目录相对于 `examples/test/conf/`，e.g. "EdgionPlugins/BasicAuth/DynamicTest"。
    pub conf_path: &'static str,
    /// scenario 执行函数（驱动整个 N-phase 状态机）。
    pub run: ScenarioRunFn,
    /// phase 名到 TestSuite 的映射。
    pub phases: &'static [PhaseSuite],
}

impl Scenario {
    /// 查找某个 phase 对应的 suite 工厂。
    pub fn phase_suite(&self, phase: &str) -> Option<&PhaseSuite> {
        self.phases.iter().find(|p| p.name == phase)
    }
}
