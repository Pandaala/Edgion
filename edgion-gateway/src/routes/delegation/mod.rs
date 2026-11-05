//! Gateway-side route delegation support shared by HTTPRoute and GRPCRoute.

pub mod descent;
pub(crate) mod index;
pub mod inherit;
pub mod slot;

pub use descent::{DelegatedMatch, DescentState};
pub use inherit::InheritedRuleValues;
pub use slot::{accept_child_keys, DelegationSlot, DelegationSlotTable, RouteParentView};
