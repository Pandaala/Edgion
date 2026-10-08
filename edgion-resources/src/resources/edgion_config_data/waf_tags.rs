//! Explicit local metadata inspection. No catalog is attached to a serving generation.

use std::io::{self, Write};

use edgion_waf::seclang::{
    compile_rule_bundles_with_selection, rule_unit_tags, CompilerError, FrontendLimits, RuleBundleMember,
    RuleBundleSource, SemanticValidationContext, ValidatedSemanticDocument, WafRuleMetadataBudget,
    WafRuleSelectionObserver, WafRuleSelectionPreview, WafSelectionSummary,
};
use serde::Serialize;

use super::{ConfigEntry, EdgionConfigData, WafRuleSelection};

pub const MAX_WAF_TAG_REPORT_BYTES: usize = 64 * 1024;
const MAX_ROWS: usize = 128;
const MAX_IDS: usize = 128;
const MAX_METADATA_BYTES: usize = 8 * 1024;
const REPORT_HEAP_BOUND: usize = 256 * 1024;

#[derive(Debug, Clone, Copy, Default)]
pub enum WafTagQuery<'a> {
    #[default]
    Catalog,
    Tag(&'a str),
    Selected,
    Excluded,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WafTagBundle {
    pub digest: String,
    pub identity: Option<String>,
    pub version: Option<String>,
    pub profile: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WafTagAtom {
    pub pointer: String,
    pub raw_matches: Option<usize>,
    pub source_active_matches: Option<usize>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WafTagRow {
    pub tag: String,
    pub unit_count: usize,
    pub source_active_unit_count: usize,
    pub phase_counts: [usize; 5],
    pub root_ids: Vec<u64>,
    pub omitted_ids: usize,
    #[serde(skip)]
    last_id: Option<u64>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WafTagReport<'a> {
    pub report_version: u8,
    pub scope: &'static str,
    pub inventory_complete: bool,
    pub report_complete: bool,
    pub baseline_compilation_passed: Option<bool>,
    pub selected_compilation_passed: Option<bool>,
    pub body_admission_checked: bool,
    pub bundles: Vec<WafTagBundle>,
    pub omitted_bundle_metadata: usize,
    pub selection: &'a WafRuleSelection,
    pub summary: Option<WafSelectionSummary>,
    pub planned_selected_unit_count: Option<usize>,
    pub selected_unit_count: Option<usize>,
    pub empty_selection: Option<bool>,
    pub unmatched_atom_refs: Vec<String>,
    pub atom_match_counts: Vec<WafTagAtom>,
    pub tags: Vec<WafTagRow>,
    pub omitted_tag_occurrences: usize,
    pub omitted_id_references: usize,
    pub membership_basis: Option<&'static str>,
    pub root_ids: Vec<u64>,
    pub total_ids: Option<usize>,
    pub offset: usize,
    pub next_offset: Option<usize>,
    pub failure: Option<&'static str>,
}

impl<'a> WafTagReport<'a> {
    fn new(selection: &'a WafRuleSelection, offset: usize) -> Self {
        Self {
            report_version: 1,
            scope: "localBundles",
            inventory_complete: false,
            report_complete: false,
            baseline_compilation_passed: None,
            selected_compilation_passed: None,
            body_admission_checked: false,
            bundles: Vec::new(),
            omitted_bundle_metadata: 0,
            selection,
            summary: None,
            planned_selected_unit_count: None,
            selected_unit_count: None,
            empty_selection: None,
            unmatched_atom_refs: Vec::new(),
            atom_match_counts: Vec::new(),
            tags: Vec::new(),
            omitted_tag_occurrences: 0,
            omitted_id_references: 0,
            membership_basis: None,
            root_ids: Vec::new(),
            total_ids: None,
            offset,
            next_offset: None,
            failure: None,
        }
    }

    /// Count before allocating the output buffer. Compact JSON is the bounded wire format.
    pub fn to_json(&self) -> Result<String, &'static str> {
        let size = encoded_bytes(self).ok_or("cannot encode WAF metadata report")?;
        if size > MAX_WAF_TAG_REPORT_BYTES - 32 {
            return Err("WAF metadata report exceeds its output bound");
        }
        let mut output = Vec::new();
        output
            .try_reserve_exact(size)
            .map_err(|_| "cannot allocate WAF metadata report")?;
        serde_json::to_writer(&mut output, self).map_err(|_| "cannot encode WAF metadata report")?;
        String::from_utf8(output).map_err(|_| "cannot encode WAF metadata report")
    }
}

struct ByteCounter {
    bytes: usize,
}
impl Write for ByteCounter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.bytes = self
            .bytes
            .checked_add(bytes.len())
            .ok_or_else(|| io::Error::other("report size overflow"))?;
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
fn encoded_bytes(value: &impl Serialize) -> Option<usize> {
    let mut counter = ByteCounter { bytes: 0 };
    serde_json::to_writer(&mut counter, value).ok()?;
    Some(counter.bytes)
}
fn copy(value: &str) -> Result<String, CompilerError> {
    let mut output = String::new();
    output.try_reserve_exact(value.len()).map_err(|_| allocation())?;
    output.push_str(value);
    Ok(output)
}
fn allocation() -> CompilerError {
    CompilerError::Allocation {
        operation: "local WAF metadata report",
    }
}

struct Observer<'a, 'b> {
    report: &'a mut WafTagReport<'b>,
    query: WafTagQuery<'a>,
    output_budget: usize,
}

impl WafRuleSelectionObserver for Observer<'_, '_> {
    fn inventory(
        &mut self,
        document: &ValidatedSemanticDocument<'_>,
        preview: WafRuleSelectionPreview<'_>,
        budget: &mut WafRuleMetadataBudget<'_>,
    ) -> Result<(), CompilerError> {
        self.report.summary = Some(preview.summary);
        self.report.planned_selected_unit_count = Some(preview.summary.selected_units);
        for (atom, (raw, active)) in self
            .report
            .atom_match_counts
            .iter_mut()
            .zip(preview.raw_atom_matches.iter().zip(preview.active_atom_matches))
        {
            atom.raw_matches = Some(*raw);
            atom.source_active_matches = Some(*active);
            if *raw == 0 {
                self.report.unmatched_atom_refs.push(copy(&atom.pointer)?);
            }
        }
        self.report.tags.try_reserve_exact(MAX_ROWS).map_err(|_| allocation())?;
        self.report
            .root_ids
            .try_reserve_exact(MAX_IDS)
            .map_err(|_| allocation())?;
        let mut id_references = 0;
        let mut total_ids = 0;
        for unit in document.rule_units_by_id() {
            budget.charge_records(1)?;
            // The mask follows source ordinals; use the existing canonical ordinal lookup.
            let selected = preview.selected[unit.ordinal()];
            let query_match = match self.query {
                WafTagQuery::Catalog => false,
                WafTagQuery::Selected => selected,
                WafTagQuery::Excluded => unit.is_active() && !selected,
                WafTagQuery::Tag(expected) => {
                    let mut found = false;
                    for tag in rule_unit_tags(unit) {
                        budget.charge_records(1)?;
                        budget.charge_bytes(tag.len().max(expected.len()))?;
                        found |= tag == expected;
                    }
                    found
                }
            };
            if query_match {
                if total_ids >= self.report.offset && self.report.root_ids.len() < MAX_IDS {
                    self.report.root_ids.push(unit.id());
                }
                total_ids += 1;
            }
            if !matches!(self.query, WafTagQuery::Catalog) {
                continue;
            }
            for tag in rule_unit_tags(unit) {
                budget.charge_records(1)?;
                let mut row_index = None;
                for (index, row) in self.report.tags.iter().enumerate() {
                    budget.charge_records(1)?;
                    budget.charge_bytes(tag.len().max(row.tag.len()))?;
                    if row.tag == tag {
                        row_index = Some(index);
                        break;
                    }
                }
                let index = if let Some(index) = row_index {
                    index
                } else {
                    // Charge exact escaping before retaining an identifier. Reserve maximum numeric widths.
                    let row_bytes = encoded_bytes(&tag)
                        .ok_or_else(allocation)?
                        .checked_add(256)
                        .ok_or_else(allocation)?;
                    if tag.len() > 256 || self.report.tags.len() == MAX_ROWS || row_bytes > self.output_budget {
                        self.report.omitted_tag_occurrences += 1;
                        continue;
                    }
                    self.output_budget -= row_bytes;
                    self.report.tags.push(WafTagRow {
                        tag: copy(tag)?,
                        unit_count: 0,
                        source_active_unit_count: 0,
                        phase_counts: [0; 5],
                        root_ids: Vec::new(),
                        omitted_ids: 0,
                        last_id: None,
                    });
                    self.report.tags.len() - 1
                };
                let row = &mut self.report.tags[index];
                if row.last_id == Some(unit.id()) {
                    continue;
                }
                row.last_id = Some(unit.id());
                row.unit_count += 1;
                row.source_active_unit_count += usize::from(unit.is_active());
                row.phase_counts[unit.root().resolved_state().phase() as usize - 1] += 1;
                if id_references < MAX_IDS {
                    row.root_ids.try_reserve_exact(1).map_err(|_| allocation())?;
                    row.root_ids.push(unit.id());
                    id_references += 1;
                } else {
                    row.omitted_ids += 1;
                    self.report.omitted_id_references += 1;
                }
            }
        }
        self.report.tags.sort_unstable_by(|a, b| a.tag.cmp(&b.tag));
        self.report.inventory_complete = true;
        if !matches!(self.query, WafTagQuery::Catalog) {
            self.report.total_ids = Some(total_ids);
            self.report.membership_basis = Some(if matches!(self.query, WafTagQuery::Tag(_)) {
                "semanticInventory"
            } else {
                "semanticPlan"
            });
            let end = self.report.offset.saturating_add(self.report.root_ids.len());
            self.report.next_offset = (end < total_ids).then_some(end);
        }
        self.report.report_complete = self.report.omitted_bundle_metadata == 0
            && self.report.omitted_tag_occurrences == 0
            && self.report.omitted_id_references == 0;
        Ok(())
    }
    fn compilation_started(&mut self, baseline: bool) {
        if baseline {
            self.report.baseline_compilation_passed = Some(false);
        } else {
            self.report.selected_compilation_passed = Some(false);
        }
    }
    fn compilation_passed(&mut self, baseline: bool) {
        if baseline {
            self.report.baseline_compilation_passed = Some(true);
        } else {
            self.report.selected_compilation_passed = Some(true);
            self.report.selected_unit_count = self.report.planned_selected_unit_count;
            self.report.empty_selection = self.report.selected_unit_count.map(|count| count == 0);
            if self.report.membership_basis == Some("semanticPlan") {
                self.report.membership_basis = Some("compiledSelection");
            }
        }
    }
}

/// Inspect caller-provided resources with no store, filesystem Include, or network access.
pub fn inspect_local_waf_tags<'a>(
    resources: &[EdgionConfigData],
    selection: &'a WafRuleSelection,
    query: WafTagQuery<'_>,
    offset: usize,
    context: SemanticValidationContext,
) -> WafTagReport<'a> {
    let mut report = WafTagReport::new(selection, offset);
    if resources.is_empty() || resources.len() > 16 || (offset != 0 && matches!(query, WafTagQuery::Catalog)) {
        report.failure = Some("invalid_input");
        return report;
    }
    let Ok(program) = selection.prepare() else {
        report.failure = Some("resource_limit");
        return report;
    };
    let mut metadata_bytes = 0;
    let mut identities = Vec::new();
    let mut inputs = Vec::new();
    if report.bundles.try_reserve_exact(resources.len()).is_err()
        || identities.try_reserve_exact(resources.len()).is_err()
        || inputs.try_reserve_exact(resources.len()).is_err()
        || report
            .atom_match_counts
            .try_reserve_exact(program.atom_refs().len())
            .is_err()
        || report
            .unmatched_atom_refs
            .try_reserve_exact(program.atom_refs().len())
            .is_err()
    {
        report.failure = Some("resource_limit");
        return report;
    }
    for (index, resource) in resources.iter().enumerate() {
        let ConfigEntry::WafRuleBundle(bundle) = &resource.spec.data else {
            report.failure = Some("invalid_input");
            return report;
        };
        if !resource.spec.enable
            || resource.spec.active.is_some()
            || !resource.spec.unknown_fields.is_empty()
            || bundle.validate().is_err()
            || resources[..index].iter().any(|prior| {
                prior.metadata.name == resource.metadata.name
                    && prior.metadata.namespace.as_deref().unwrap_or("default")
                        == resource.metadata.namespace.as_deref().unwrap_or("default")
            })
        {
            report.failure = Some("invalid_input");
            return report;
        }
        let digest = bundle.compute_digest();
        let allocated = (|| -> Result<_, CompilerError> {
            let namespace = resource.metadata.namespace.as_deref().unwrap_or("default");
            let name = resource.metadata.name.as_deref().unwrap_or("unknown");
            let identity = if name.len() + namespace.len() <= 4096 {
                let mut value = String::new();
                value
                    .try_reserve_exact(namespace.len() + 1 + name.len())
                    .map_err(|_| allocation())?;
                value.push_str(namespace);
                value.push('/');
                value.push_str(name);
                Some(value)
            } else {
                None
            };
            let metadata = (&identity, &bundle.version, &bundle.profile);
            let bytes = encoded_bytes(&metadata).unwrap_or(MAX_METADATA_BYTES + 1);
            let keep = identity.is_some() && bytes <= MAX_METADATA_BYTES.saturating_sub(metadata_bytes);
            let row = WafTagBundle {
                digest: copy(&digest)?,
                identity: if keep { identity } else { None },
                version: if keep { Some(copy(&bundle.version)?) } else { None },
                profile: if keep { Some(copy(&bundle.profile)?) } else { None },
            };
            let mut local_identity = String::new();
            local_identity.try_reserve_exact(80).map_err(|_| allocation())?;
            std::fmt::Write::write_fmt(&mut local_identity, format_args!("local-{index}@{digest}"))
                .map_err(|_| allocation())?;
            let input = (
                collect_views(bundle.roots.iter().map(String::as_str))?,
                collect_views(bundle.rules.iter().map(|member| RuleBundleMember {
                    path: &member.name,
                    bytes: member.content.as_bytes(),
                }))?,
                collect_views(bundle.phrase_assets.iter().map(|member| RuleBundleMember {
                    path: &member.name,
                    bytes: member.content.as_bytes(),
                }))?,
            );
            Ok((row, local_identity, input, keep, bytes))
        })();
        let Ok((row, identity, input, keep, bytes)) = allocated else {
            report.failure = Some("resource_limit");
            return report;
        };
        if keep {
            metadata_bytes += bytes;
        } else {
            report.omitted_bundle_metadata += 1;
        }
        report.bundles.push(row);
        identities.push(identity);
        inputs.push(input);
    }
    for atom in program.atom_refs() {
        let Ok(pointer) = copy(&atom.pointer) else {
            report.failure = Some("resource_limit");
            return report;
        };
        report.atom_match_counts.push(WafTagAtom {
            pointer,
            raw_matches: None,
            source_active_matches: None,
        });
    }
    let sources = collect_views(identities.iter().zip(&inputs).map(
        |(identity, (entrypoints, rules, phrase_assets))| RuleBundleSource {
            identity,
            entrypoints,
            rules,
            phrase_assets,
        },
    ));
    let Ok(sources) = sources else {
        report.failure = Some("resource_limit");
        return report;
    };
    // Base document + atom/unmatched pointers + maximum numeric widths + ID references.
    let base_bytes = encoded_bytes(&report).unwrap_or(MAX_WAF_TAG_REPORT_BYTES)
        + program
            .atom_refs()
            .iter()
            .map(|atom| encoded_bytes(&atom.pointer).unwrap_or(MAX_WAF_TAG_REPORT_BYTES) + 100)
            .sum::<usize>()
        + 4096;
    if base_bytes > MAX_WAF_TAG_REPORT_BYTES {
        report.failure = Some("resource_limit");
        return report;
    }
    let mut observer = Observer {
        report: &mut report,
        query,
        output_budget: MAX_WAF_TAG_REPORT_BYTES - base_bytes,
    };
    let adapter_bytes = inputs.capacity()
        * std::mem::size_of::<(Vec<&str>, Vec<RuleBundleMember<'_>>, Vec<RuleBundleMember<'_>>)>()
        + inputs
            .iter()
            .map(|(entries, rules, assets)| {
                entries.capacity() * std::mem::size_of::<&str>()
                    + (rules.capacity() + assets.capacity()) * std::mem::size_of::<RuleBundleMember<'_>>()
            })
            .sum::<usize>()
        + sources.capacity() * std::mem::size_of::<RuleBundleSource<'_>>()
        + identities.capacity() * std::mem::size_of::<String>()
        + identities.iter().map(String::capacity).sum::<usize>();
    let result = compile_rule_bundles_with_selection(
        &sources,
        "",
        context,
        FrontendLimits::default(),
        &program,
        REPORT_HEAP_BOUND + adapter_bytes + selection.validate().map_or(0, |stats| stats.owned_bytes),
        |_| Ok::<_, std::convert::Infallible>(()),
        Some(&mut observer),
    );
    if let Err(error) = result {
        use edgion_waf::seclang::{FrontendError, WafSelectionCompileError};
        report.failure = Some(match error {
            WafSelectionCompileError::Frontend(FrontendError::Source(_) | FrontendError::Syntax(_)) => {
                "source_syntax_failed"
            }
            WafSelectionCompileError::Frontend(FrontendError::Semantic(_) | FrontendError::Validation(_)) => {
                "semantic_validation_failed"
            }
            WafSelectionCompileError::Frontend(FrontendError::RuleBundle(_) | FrontendError::Include(_)) => {
                "bundle_expansion_failed"
            }
            WafSelectionCompileError::Frontend(_) => "strict_compilation_failed",
            WafSelectionCompileError::Admission(never) => match never {},
        });
        if !report.inventory_complete {
            report.summary = None;
            report.planned_selected_unit_count = None;
            report.root_ids.clear();
            report.unmatched_atom_refs.clear();
            for atom in &mut report.atom_match_counts {
                atom.raw_matches = None;
                atom.source_active_matches = None;
            }
        }
    }
    report
}

fn collect_views<T>(values: impl ExactSizeIterator<Item = T>) -> Result<Vec<T>, CompilerError> {
    let mut result = Vec::new();
    result.try_reserve_exact(values.len()).map_err(|_| allocation())?;
    result.extend(values);
    Ok(result)
}

#[cfg(test)]
#[path = "../../../tests/unit/resources/edgion_config_data/waf_tags_test.rs"]
mod tests;
