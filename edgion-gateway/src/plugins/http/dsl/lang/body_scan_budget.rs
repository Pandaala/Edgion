//! Execution-local account shared by scans and opaque HTTP uploads.
//!
//! Reservations are settled explicitly only after work reports fetched bytes.
//! There is deliberately no Drop refund: cancelled blocking work may still run.

use super::body_ops::body_access_error;
use super::error::RuntimeError;

#[derive(Default)]
pub(crate) struct BodyScanBudget {
    remaining: Option<u64>,
}

impl BodyScanBudget {
    pub(crate) fn reserve(&mut self, physical_cap: u64, passes: u8) -> Result<u64, RuntimeError> {
        let budget = match self.remaining {
            Some(budget) => budget,
            None => {
                let budget = physical_cap
                    .checked_mul(u64::from(passes))
                    .ok_or_else(|| body_access_error("request body scan budget overflow"))?;
                self.remaining = Some(budget);
                budget
            }
        };
        self.remaining = Some(0);
        Ok(budget)
    }

    pub(crate) fn settle(&mut self, reserved: u64, scanned: u64) {
        self.remaining = Some(reserved.saturating_sub(scanned));
    }

    pub(crate) fn reserve_exact(&mut self, bytes: u64, physical_cap: u64, passes: u8) -> Result<(), RuntimeError> {
        let budget = match self.remaining {
            Some(budget) => budget,
            None => physical_cap
                .checked_mul(u64::from(passes))
                .ok_or_else(|| body_access_error("request body scan budget overflow"))?,
        };
        let remaining = budget
            .checked_sub(bytes)
            .ok_or_else(|| body_access_error("request body scan budget exceeded"))?;
        self.remaining = Some(remaining);
        Ok(())
    }

    pub(crate) fn refund_exact(&mut self, bytes: u64) {
        let budget = self.remaining.unwrap_or(0);
        self.remaining = Some(budget.saturating_add(bytes));
    }
}

#[cfg(test)]
#[path = "../../../../../tests/unit/plugins/http/dsl/lang/body_scan_budget_test.rs"]
mod tests;
