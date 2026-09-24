use super::OperatorRecord;
use std::mem::size_of;

pub(in crate::seclang) const fn operator_record_size_for_test() -> usize {
    size_of::<OperatorRecord<'static>>()
}
