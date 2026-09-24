#[inline]
#[cfg(test)]
fn choose_typed_retry_budget(
    selected_is_ai: bool,
    ai_budget: Option<std::sync::Arc<RetryBudget>>,
    service_budget: Option<std::sync::Arc<RetryBudget>>,
) -> Option<std::sync::Arc<RetryBudget>> {
    if selected_is_ai {
        ai_budget
    } else {
        service_budget
    }
}
