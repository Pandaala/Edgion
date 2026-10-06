//! Attributable capability findings for bounded XML value inspection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum XmlInspectionUnsupportedReason {
    Path,
    DynamicValueOperand,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct XmlInspectionUnsupported {
    pub rule_id: u64,
    pub member: usize,
    pub reason: XmlInspectionUnsupportedReason,
}
