// Scenario 注册表。新增 scenario 只需在此列表中添加一项。

use crate::dynamic::scenario::Scenario;

/// 所有已注册的 dynamic scenarios。
pub fn all() -> Vec<&'static Scenario> {
    vec![
        crate::dynamic::scenarios::gateway_dynamic::SCENARIO,
        crate::dynamic::scenarios::dsl_dynamic::SCENARIO,
        crate::dynamic::scenarios::basic_auth_dynamic::SCENARIO,
        crate::dynamic::scenarios::rate_limit_dynamic::SCENARIO,
        crate::dynamic::scenarios::observed_gen_high_freq::SCENARIO,
        crate::dynamic::scenarios::btlsp_ref_flap::SCENARIO,
        crate::dynamic::scenarios::pmd_ref_flap::SCENARIO,
        crate::dynamic::scenarios::sp_backend_change::SCENARIO,
    ]
}

/// 按 id 查找 scenario。
pub fn find(id: &str) -> Option<&'static Scenario> {
    all().into_iter().find(|s| s.id == id)
}
