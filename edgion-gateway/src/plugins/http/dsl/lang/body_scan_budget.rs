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
mod tests {
    use super::BodyScanBudget;

    #[test]
    fn exact_reservation_charges_only_fetched_bytes_after_refund() {
        let mut state = BodyScanBudget::default();

        state.reserve_exact(5, 16, 1).unwrap();
        state.refund_exact(2);
        state.reserve_exact(13, 16, 1).unwrap();

        assert!(state.reserve_exact(1, 16, 1).is_err());
    }

    #[test]
    fn exact_reservation_rejects_before_overdrawing_budget() {
        let mut state = BodyScanBudget::default();

        assert!(state.reserve_exact(17, 16, 1).is_err());
        state.reserve_exact(0, 16, 1).unwrap();
        state.reserve_exact(16, 16, 1).unwrap();
    }

    #[test]
    fn scans_and_uploads_share_remaining_bytes_without_reinitializing() {
        let mut budget = BodyScanBudget::default();
        budget.reserve_exact(5, 16, 1).unwrap();
        budget.refund_exact(2);
        let reserved = budget.reserve(128, 4).unwrap();
        assert_eq!(reserved, 13);
        budget.settle(reserved, 4);
        budget.reserve_exact(9, 128, 4).unwrap();
        assert!(budget.reserve_exact(1, 128, 4).is_err());
    }

    #[test]
    fn failed_or_unsettled_scan_does_not_refund_its_reservation() {
        let mut budget = BodyScanBudget::default();
        assert_eq!(budget.reserve(16, 2).unwrap(), 32);
        assert_eq!(budget.reserve(16, 2).unwrap(), 0);
        assert!(budget.reserve_exact(1, 16, 2).is_err());
    }

    #[test]
    fn budget_initialization_overflow_does_not_create_credit() {
        let mut budget = BodyScanBudget::default();
        assert!(budget.reserve(u64::MAX, 2).is_err());
        assert!(budget.reserve_exact(0, u64::MAX, 2).is_err());
        assert_eq!(budget.reserve(16, 1).unwrap(), 16);
    }
}
