// Scenario 注册表。新增 scenario 只需在此列表中添加一项。

use crate::dynamic::scenario::Scenario;

/// 所有已注册的 dynamic scenarios。
pub fn all() -> Vec<&'static Scenario> {
    vec![
        crate::dynamic::scenarios::gateway_dynamic::SCENARIO,
        crate::dynamic::scenarios::dsl_dynamic::SCENARIO,
        // Disabled until plugin hot-reload bugs are fixed.
        // tasks/todo/bug-basic-auth-secretref-removal-hot-reload.md
        // crate::dynamic::scenarios::basic_auth_dynamic::SCENARIO,
        // tasks/todo/bug-rate-limit-on-missing-key-hot-reload.md
        // crate::dynamic::scenarios::rate_limit_dynamic::SCENARIO,
    ]
}

/// 按 id 查找 scenario。
pub fn find(id: &str) -> Option<&'static Scenario> {
    all().into_iter().find(|s| s.id == id)
}
