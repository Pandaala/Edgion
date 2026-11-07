//! Compatibility report: unavailable-capability findings and per-rule summaries for a rule set.

use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CompatibilityFinding {
    capability: UnavailableCapability,
    location: SemanticLocation,
    rule_id: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CompatibilityRuleSummary {
    rule_id: u64,
    capability_mask: u8,
}

impl CompatibilityRuleSummary {
    pub const fn rule_id(self) -> u64 {
        self.rule_id
    }

    pub const fn has_capability(self, capability: UnavailableCapability) -> bool {
        self.capability_mask & (1_u8 << capability_index(capability)) != 0
    }
}

const fn capability_index(capability: UnavailableCapability) -> usize {
    match capability {
        UnavailableCapability::FullResponseBody => 0,
        UnavailableCapability::Ctl => 1,
        UnavailableCapability::InitCol => 2,
    }
}

impl CompatibilityFinding {
    pub const fn capability(self) -> UnavailableCapability {
        self.capability
    }

    pub const fn location(self) -> SemanticLocation {
        self.location
    }

    pub const fn rule_id(self) -> Option<u64> {
        self.rule_id
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompatibilityReport {
    include_metrics: IncludeMetrics,
    directive_count: usize,
    rule_count: usize,
    requirement_count: usize,
    unavailable_counts: [usize; 3],
    rule_unit_count: usize,
    unavailable_rule_unit_count: usize,
    unattributed_findings: usize,
    pub(super) rule_summaries: Vec<CompatibilityRuleSummary>,
    omitted_rule_summaries: usize,
    pub(super) findings: Vec<CompatibilityFinding>,
    omitted_findings: usize,
}

impl CompatibilityReport {
    pub(super) fn from_draft(
        draft: &SemanticDraft<'_>,
        include_metrics: IncludeMetrics,
        limits: CompatibilityLimits,
    ) -> Result<Self, CompatibilityError> {
        let mut root_ids = Vec::new();
        root_ids
            .try_reserve_exact(draft.final_rules().len())
            .map_err(|_| CompatibilityError::Allocation)?;
        for rule in draft.final_rules() {
            if matches!(rule.role(), FinalRuleRole::ChainChild { .. }) {
                continue;
            }
            let Some(id) = rule.fold().id() else {
                continue;
            };
            let location = rule.location();
            root_ids.push((location.document_index(), location.directive_index(), id));
        }
        root_ids.sort_unstable_by_key(|(document, directive, _)| (*document, *directive));

        let mut attributions = Vec::new();
        attributions
            .try_reserve_exact(draft.final_rules().len())
            .map_err(|_| CompatibilityError::Allocation)?;
        for rule in draft.final_rules() {
            let unit_location = match rule.role() {
                FinalRuleRole::ChainChild { root, .. } => root,
                _ => rule.location(),
            };
            let Ok(index) = root_ids.binary_search_by_key(
                &(unit_location.document_index(), unit_location.directive_index()),
                |(document, directive, _)| (*document, *directive),
            ) else {
                continue;
            };
            let location = rule.location();
            attributions.push((location.document_index(), location.directive_index(), root_ids[index].2));
        }
        attributions.sort_unstable_by_key(|(document, directive, _)| (*document, *directive));

        let mut findings = Vec::new();
        findings
            .try_reserve_exact(limits.max_findings.min(draft.unavailable_capabilities().len()))
            .map_err(|_| CompatibilityError::Allocation)?;
        let mut unavailable_counts = [0_usize; 3];
        let mut omitted_findings = 0_usize;
        let mut unavailable_rule_capabilities = Vec::new();
        unavailable_rule_capabilities
            .try_reserve_exact(draft.unavailable_capabilities().len())
            .map_err(|_| CompatibilityError::Allocation)?;
        let mut unattributed_findings = 0_usize;
        for record in draft.unavailable_capabilities() {
            let index = capability_index(record.capability());
            unavailable_counts[index] = unavailable_counts[index]
                .checked_add(1)
                .ok_or(CompatibilityError::Arithmetic)?;
            let location = record.location();
            let rule_id = attributions
                .binary_search_by_key(
                    &(location.document_index(), location.directive_index()),
                    |(document, directive, _)| (*document, *directive),
                )
                .ok()
                .map(|index| attributions[index].2);
            if let Some(rule_id) = rule_id {
                unavailable_rule_capabilities.push((rule_id, index));
            } else {
                unattributed_findings = unattributed_findings
                    .checked_add(1)
                    .ok_or(CompatibilityError::Arithmetic)?;
            }
            if findings.len() < limits.max_findings {
                findings.push(CompatibilityFinding {
                    capability: record.capability(),
                    location,
                    rule_id,
                });
            } else {
                omitted_findings = omitted_findings.checked_add(1).ok_or(CompatibilityError::Arithmetic)?;
            }
        }
        unavailable_rule_capabilities.sort_unstable();
        let mut rule_summaries = Vec::new();
        rule_summaries
            .try_reserve_exact(limits.max_rule_summaries.min(unavailable_rule_capabilities.len()))
            .map_err(|_| CompatibilityError::Allocation)?;
        let mut unavailable_rule_unit_count = 0_usize;
        let mut omitted_rule_summaries = 0_usize;
        let mut cursor = 0_usize;
        while cursor < unavailable_rule_capabilities.len() {
            let rule_id = unavailable_rule_capabilities[cursor].0;
            let mut capability_mask = 0_u8;
            while cursor < unavailable_rule_capabilities.len() && unavailable_rule_capabilities[cursor].0 == rule_id {
                capability_mask |= 1_u8 << unavailable_rule_capabilities[cursor].1;
                cursor += 1;
            }
            unavailable_rule_unit_count = unavailable_rule_unit_count
                .checked_add(1)
                .ok_or(CompatibilityError::Arithmetic)?;
            if rule_summaries.len() < limits.max_rule_summaries {
                rule_summaries.push(CompatibilityRuleSummary {
                    rule_id,
                    capability_mask,
                });
            } else {
                omitted_rule_summaries = omitted_rule_summaries
                    .checked_add(1)
                    .ok_or(CompatibilityError::Arithmetic)?;
            }
        }
        Ok(Self {
            include_metrics,
            directive_count: draft.directive_count(),
            rule_count: draft.final_rules().len(),
            requirement_count: draft.requirements().len(),
            unavailable_counts,
            rule_unit_count: root_ids.len(),
            unavailable_rule_unit_count,
            unattributed_findings,
            rule_summaries,
            omitted_rule_summaries,
            findings,
            omitted_findings,
        })
    }

    pub const fn include_metrics(&self) -> IncludeMetrics {
        self.include_metrics
    }

    pub const fn directive_count(&self) -> usize {
        self.directive_count
    }

    pub const fn rule_count(&self) -> usize {
        self.rule_count
    }

    pub const fn requirement_count(&self) -> usize {
        self.requirement_count
    }

    pub const fn is_supported(&self) -> bool {
        self.unavailable_counts[0] == 0 && self.unavailable_counts[1] == 0 && self.unavailable_counts[2] == 0
    }

    pub const fn rule_unit_count(&self) -> usize {
        self.rule_unit_count
    }

    pub const fn unavailable_rule_unit_count(&self) -> usize {
        self.unavailable_rule_unit_count
    }

    pub const fn locally_executable_rule_unit_count(&self) -> usize {
        self.rule_unit_count - self.unavailable_rule_unit_count
    }

    pub const fn unattributed_findings(&self) -> usize {
        self.unattributed_findings
    }

    pub fn rule_summaries(&self) -> impl ExactSizeIterator<Item = CompatibilityRuleSummary> + '_ {
        self.rule_summaries.iter().copied()
    }

    pub const fn omitted_rule_summaries(&self) -> usize {
        self.omitted_rule_summaries
    }

    pub const fn unavailable_count(&self, capability: UnavailableCapability) -> usize {
        self.unavailable_counts[capability_index(capability)]
    }

    pub fn findings(&self) -> impl ExactSizeIterator<Item = CompatibilityFinding> + '_ {
        self.findings.iter().copied()
    }

    pub const fn omitted_findings(&self) -> usize {
        self.omitted_findings
    }
}

#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum CompatibilityError {
    #[error("compatibility report arithmetic overflowed")]
    Arithmetic,
    #[error("cannot allocate bounded compatibility findings")]
    Allocation,
    #[error("compatibility findings could not be attributed to rule units")]
    UnattributedRule,
    #[error("compatibility rule summaries exceed the configured limit of {limit}")]
    RuleSummaryLimit { limit: usize },
}
