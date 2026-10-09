//! Compile the conservative boundary of optional retained aggregate checks.
use super::*;
use crate::seclang::{OperatorKind, OperatorOperandView, ValidatedRuleNodeView, ValidatedRuleUnitView};

#[path = "expensive_check_signatures.rs"]
mod signatures;

pub(super) enum SelectorSignature {
    None,
    Regex(&'static str),
}
pub(super) enum OperatorSignature {
    Regex(&'static str),
    Integer(i64, &'static str),
    String(&'static str),
}
pub(super) enum ActionSignature {
    NoArgument,
    Local(LocalActionValue),
    Literal(&'static str),
    Template(&'static str),
    Transformation(TransformationKind),
    SetVar(SetVarOperation, &'static str, &'static str),
}
pub(super) struct MemberSignature {
    collection: CollectionKind,
    modifier: TargetModifier,
    selector: SelectorSignature,
    operator: OperatorKind,
    negated: bool,
    operand: OperatorSignature,
    actions: &'static [(&'static str, ActionSignature)],
    transformations: &'static [TransformationKind],
}

#[derive(Clone, Copy)]
enum AggregateFamily {
    Hpp,
    Multipart,
}
#[derive(Default)]
pub(super) struct OptionalChecks {
    pub(super) hpp: bool,
    pub(super) multipart: bool,
}
impl OptionalChecks {
    pub(super) fn skips_unit(&self, id: u64) -> bool {
        (self.hpp && matches!(id, 921170 | 921180)) || (self.multipart && matches!(id, 922140 | 922110))
    }
}

struct Proof<'a> {
    work: &'a mut CanonicalWork,
    limits: CompilerLimits,
}
impl Proof<'_> {
    fn record(&mut self) -> Result<(), CompilerError> {
        self.work.charge_records(1, self.limits)
    }
    fn equal(&mut self, actual: &str, expected: &str) -> Result<bool, CompilerError> {
        self.record()?;
        self.work
            .charge_comparison_bytes(actual.len().min(expected.len()), self.limits)?;
        Ok(actual == expected)
    }
    fn histogram_key(&mut self, key: &str) -> Result<bool, CompilerError> {
        const PREFIX: &str = "paramcounter_";
        self.record()?;
        self.work
            .charge_comparison_bytes(key.len().min(PREFIX.len()), self.limits)?;
        Ok(key
            .as_bytes()
            .get(..PREFIX.len())
            .is_some_and(|start| start.eq_ignore_ascii_case(PREFIX.as_bytes())))
    }
    fn template_matches<'a, 's: 'a>(
        &mut self,
        segments: impl Iterator<Item = (TemplateSegmentKind<'a, 's>, &'a str)>,
        expected: &str,
    ) -> Result<bool, CompilerError> {
        let mut rest = expected;
        for (kind, text) in segments {
            self.record()?;
            let (piece, remaining) = if rest.starts_with("%{") {
                let Some(end) = rest.find('}') else { return Ok(false) };
                rest.split_at(end + 1)
            } else {
                rest.split_at(rest.find("%{").unwrap_or(rest.len()))
            };
            if !self.equal(text, piece)? {
                return Ok(false);
            }
            match kind {
                TemplateSegmentKind::Literal => {
                    if piece.starts_with("%{") {
                        return Ok(false);
                    }
                }
                TemplateSegmentKind::Expansion(expansion) => {
                    let Some(body) = piece.strip_prefix("%{").and_then(|s| s.strip_suffix('}')) else {
                        return Ok(false);
                    };
                    let (source, key) = if let Some(key) = body.strip_prefix("tx.").or_else(|| body.strip_prefix("TX."))
                    {
                        (MacroSource::Tx, Some(key))
                    } else if let Some(key) = body.strip_prefix("request_headers.") {
                        (MacroSource::RequestHeaders, Some(key))
                    } else if body == "MATCHED_VAR_NAME" {
                        (MacroSource::MatchedVarName, None)
                    } else if body == "MATCHED_VAR" {
                        (MacroSource::MatchedVar, None)
                    } else {
                        return Ok(false);
                    };
                    if expansion.source() != source {
                        return Ok(false);
                    }
                    match (expansion.key(), key) {
                        (None, None) => (),
                        (Some(actual), Some(expected)) if self.equal(actual.text(), expected)? => (),
                        _ => return Ok(false),
                    }
                }
            }
            rest = remaining;
        }
        Ok(rest.is_empty())
    }
    fn member_matches(
        &mut self,
        member: ValidatedRuleNodeView<'_, '_>,
        expected: &MemberSignature,
    ) -> Result<bool, CompilerError> {
        self.record()?;
        if member.skip_after_marker().is_some() {
            return Ok(false);
        }
        let mut targets = member.final_targets();
        let Some(target) = targets.next() else { return Ok(false) };
        if targets.next().is_some()
            || target.collection() != expected.collection
            || target.modifier() != expected.modifier
        {
            return Ok(false);
        }
        let selector_ok = match (&expected.selector, target.selector_kind()) {
            (SelectorSignature::None, SelectorKind::None) => true,
            (SelectorSignature::Regex(source), SelectorKind::Regex(RegexStyle::Bare)) => match target.regex_body() {
                Some(actual) => self.equal(actual.text(), source)?,
                None => false,
            },
            _ => false,
        };
        if !selector_ok {
            return Ok(false);
        }
        let Some(operator) = member.operator() else {
            return Ok(false);
        };
        if operator.kind() != expected.operator || operator.negated() != expected.negated {
            return Ok(false);
        }
        let operand_ok = match (&expected.operand, operator.operand()) {
            (OperatorSignature::Regex(source), OperatorOperandView::Regex(actual)) => {
                self.equal(actual.text(), source)?
            }
            (OperatorSignature::Integer(value, text), OperatorOperandView::IntegerTemplate { constant, template }) => {
                constant == Some(*value) && template.expansion_count() == 0 && self.equal(template.text(), text)?
            }
            (OperatorSignature::String(source), OperatorOperandView::StringTemplate(template)) => {
                self.template_matches(template.segments().map(|s| (s.kind(), s.text())), source)?
            }
            _ => false,
        };
        if !operand_ok {
            return Ok(false);
        }
        let mut transforms = member.effective_transformations();
        for expected in expected.transformations {
            self.record()?;
            if transforms.next().and_then(|a| a.transformation()) != Some(*expected) {
                return Ok(false);
            }
        }
        if transforms.next().is_some() {
            return Ok(false);
        }
        let mut actions = member.actions();
        for (name, expected) in expected.actions {
            self.record()?;
            let Some(actual) = actions.next() else { return Ok(false) };
            let canonical_name = match actual.kind() {
                ActionEnvelopeKind::Action(kind) => kind.as_str(),
                ActionEnvelopeKind::Transformation => "t",
            };
            if !self.equal(canonical_name, name)? {
                return Ok(false);
            }
            let matches = match (expected, actual.operand()) {
                (ActionSignature::NoArgument, ActionOperandView::NoArgument) => true,
                (ActionSignature::Local(expected), ActionOperandView::Local(actual)) => actual == *expected,
                (ActionSignature::Literal(expected), ActionOperandView::Literal(actual)) => {
                    self.equal(actual.text(), expected)?
                }
                (ActionSignature::Template(expected), ActionOperandView::Template(actual)) => {
                    self.template_matches(actual.segments().map(|s| (s.kind(), s.text())), expected)?
                }
                (ActionSignature::Transformation(expected), ActionOperandView::Transformation(actual)) => {
                    actual == *expected
                }
                (ActionSignature::SetVar(operation, key, value), ActionOperandView::SetVar(actual)) => {
                    if actual.collection() != CollectionKind::Tx || actual.operation() != *operation {
                        return Ok(false);
                    }
                    let key_matches =
                        self.template_matches(actual.key().segments().map(|s| (s.kind(), s.text())), key)?;
                    let Some(actual_value) = actual.value() else {
                        return Ok(false);
                    };
                    key_matches
                        && self.template_matches(actual_value.segments().map(|s| (s.kind(), s.text())), value)?
                }
                _ => false,
            };
            if !matches {
                return Ok(false);
            }
        }
        Ok(actions.next().is_none())
    }
    fn unit_matches(
        &mut self,
        unit: ValidatedRuleUnitView<'_, '_>,
        expected: &[MemberSignature],
    ) -> Result<bool, CompilerError> {
        self.record()?;
        let state = unit.root().resolved_state();
        let root = &expected[0];
        let expected_phase = root.actions.iter().find_map(|(_, action)| match action {
            ActionSignature::Local(LocalActionValue::Phase(phase)) => Some(*phase),
            _ => None,
        });
        let quiet = root.actions.iter().any(|(name, _)| *name == "nolog");
        let pass = root.actions.iter().any(|(name, _)| *name == "pass");
        if Some(state.phase()) != expected_phase
            || state.status().is_some()
            || state.audit_log() == quiet
            || state.log() == quiet
            || (pass && state.disruptive() != ResolvedDisruptive::Pass)
        {
            return Ok(false);
        }
        let mut members = unit.members();
        for expected in expected {
            let Some(member) = members.next() else { return Ok(false) };
            if !self.member_matches(member, expected)? {
                return Ok(false);
            }
        }
        Ok(members.next().is_none())
    }
    fn reads_aggregate<'a, 's: 'a>(
        &mut self,
        segments: impl Iterator<Item = TemplateSegmentKind<'a, 's>>,
        observer_may_alias: bool,
        family: AggregateFamily,
    ) -> Result<bool, CompilerError> {
        for segment in segments {
            self.record()?;
            if let TemplateSegmentKind::Expansion(expansion) = segment {
                match expansion.source() {
                    MacroSource::Tx => {
                        let Some(key) = expansion.key() else { return Ok(true) };
                        if self.aggregate_key(key.text(), family, false)? {
                            return Ok(true);
                        }
                    }
                    MacroSource::MatchedVar | MacroSource::MatchedVarName if observer_may_alias => return Ok(true),
                    _ => (),
                }
            }
        }
        Ok(false)
    }
    fn aggregate_key(&mut self, key: &str, family: AggregateFamily, writer: bool) -> Result<bool, CompilerError> {
        if !writer && matches!(family, AggregateFamily::Hpp) {
            return self.histogram_key(key);
        }
        let stems: &[&str] = match family {
            AggregateFamily::Hpp => &["paramcounter_"],
            AggregateFamily::Multipart => &["multipart_headers_content_types", "multipart_headers_content_counter"],
        };
        for stem in stems {
            self.record()?;
            if writer {
                self.work.charge_comparison_bytes(
                    key.len()
                        .saturating_sub(stem.len())
                        .saturating_add(1)
                        .checked_mul(stem.len())
                        .ok_or(CompilerError::PreflightArithmetic)?,
                    self.limits,
                )?;
                if key
                    .as_bytes()
                    .windows(stem.len())
                    .any(|part| part.eq_ignore_ascii_case(stem.as_bytes()))
                {
                    return Ok(true);
                }
            } else {
                self.work
                    .charge_comparison_bytes(key.len().min(stem.len()), self.limits)?;
                if key
                    .as_bytes()
                    .get(..stem.len())
                    .is_some_and(|part| part.eq_ignore_ascii_case(stem.as_bytes()))
                {
                    return Ok(true);
                }
            }
        }
        Ok(false)
    }
    fn writer_may_alias(
        &mut self,
        key: TemplateSliceView<'_, '_>,
        family: AggregateFamily,
    ) -> Result<bool, CompilerError> {
        self.record()?;
        let mut segments = key.segments();
        let Some(first) = segments.next() else { return Ok(true) };
        if !matches!(first.kind(), TemplateSegmentKind::Literal) || segments.next().is_some() {
            return Ok(true);
        }
        // Both checkers are unanchored. Different leading prefixes are not proof.
        self.aggregate_key(first.text(), family, true)
    }
    fn external_member_alias(
        &mut self,
        member: ValidatedRuleNodeView<'_, '_>,
        family: AggregateFamily,
        observer_may_alias: bool,
    ) -> Result<bool, CompilerError> {
        let mut has_positive_target = false;
        for target in member.final_targets() {
            self.record()?;
            has_positive_target |= target.modifier() != TargetModifier::Exclude;
            if matches!(
                target.collection(),
                CollectionKind::MatchedVar | CollectionKind::MatchedVars
            ) && observer_may_alias
            {
                return Ok(true);
            }
            if target.collection() != CollectionKind::Tx {
                continue;
            }
            match target.selector_kind() {
                SelectorKind::Literal => {
                    let Some(key) = target.selector() else { return Ok(true) };
                    if self.aggregate_key(key.text(), family, false)? {
                        return Ok(true);
                    }
                }
                SelectorKind::Regex(RegexStyle::Bare) => {
                    let Some(source) = target.regex_body() else {
                        return Ok(true);
                    };
                    // Exact anchored literal patterns. No unexamined regex suffix.
                    if matches!(family, AggregateFamily::Multipart)
                        || (!self.equal(source.text(), "^header_name_920450_")?
                            && !self.equal(source.text(), "^header_name_920451_")?)
                    {
                        return Ok(true);
                    }
                }
                _ => return Ok(true),
            }
        }
        if let Some(operator) = member.operator() {
            match operator.operand() {
                OperatorOperandView::StringTemplate(t) | OperatorOperandView::IntegerTemplate { template: t, .. }
                    if self.reads_aggregate(t.segments().map(|s| s.kind()), observer_may_alias, family)? =>
                {
                    return Ok(true);
                }
                _ => (),
            }
        }
        for action in member.actions() {
            self.record()?;
            match action.operand() {
                ActionOperandView::Template(t) => {
                    if self.reads_aggregate(
                        t.segments().map(|s| s.kind()),
                        observer_may_alias && !has_positive_target,
                        family,
                    )? {
                        return Ok(true);
                    }
                }
                ActionOperandView::SetVar(v) => {
                    if v.collection() == CollectionKind::Tx && self.writer_may_alias(v.key(), family)? {
                        return Ok(true);
                    }
                    if self.reads_aggregate(
                        v.key().segments().map(|s| s.kind()),
                        observer_may_alias && !has_positive_target,
                        family,
                    )? {
                        return Ok(true);
                    }
                    if let Some(value) = v.value() {
                        if self.reads_aggregate(
                            value.segments().map(|s| s.kind()),
                            observer_may_alias && !has_positive_target,
                            family,
                        )? {
                            return Ok(true);
                        }
                    }
                }
                ActionOperandView::InitCol(_) => return Ok(true),
                _ => (),
            }
        }
        Ok(false)
    }

    fn root_has_local_observers(&mut self, root: ValidatedRuleNodeView<'_, '_>) -> Result<bool, CompilerError> {
        let mut positive = false;
        for target in root.final_targets() {
            self.record()?;
            if matches!(
                target.collection(),
                CollectionKind::MatchedVar | CollectionKind::MatchedVars
            ) {
                return Ok(false);
            }
            positive |= target.modifier() != TargetModifier::Exclude;
        }
        Ok(positive)
    }
}

pub(super) fn classify(
    validated: &ValidatedSemanticDocument<'_>,
    captures: &[CompiledCaptureRegexProgram],
    work: &mut CanonicalWork,
    limits: CompilerLimits,
) -> Result<OptionalChecks, CompilerError> {
    let mut proof = Proof { work, limits };
    let signatures = [
        (920450, signatures::UNIT_920450),
        (920451, signatures::UNIT_920451),
        (921170, signatures::UNIT_921170),
        (921180, signatures::UNIT_921180),
        (922140, signatures::UNIT_922140),
        (922150, signatures::UNIT_922150),
        (922110, signatures::UNIT_922110),
        (931130, signatures::UNIT_931130),
    ];
    let mut canonical = [false; 8];
    for (index, (id, shape)) in signatures.iter().enumerate() {
        if let Some(unit) = validated.rule_by_id(*id) {
            canonical[index] = proof.unit_matches(unit, shape)?;
        }
    }
    let mut optional = OptionalChecks {
        hpp: canonical[2] && canonical[3],
        multipart: canonical[4] && canonical[5] && canonical[6],
    };
    for unit in validated.active_rule_units() {
        if !optional.hpp && !optional.multipart {
            break;
        }
        proof.record()?;
        if signatures
            .iter()
            .zip(canonical)
            .any(|((id, _), exact)| exact && *id == unit.id())
        {
            continue;
        }
        // First-member evaluation and staged unit clones hide previous-unit observers.
        // A positive independent root therefore supplies only local chain observations.
        // Direct TX namespace and writer checks remain necessary on every member.
        let local_observers = proof.root_has_local_observers(unit.root())?;
        for (index, member) in unit.members().enumerate() {
            let observer_may_alias = index == 0 || !local_observers;
            if optional.hpp && proof.external_member_alias(member, AggregateFamily::Hpp, observer_may_alias)? {
                optional.hpp = false;
            }
            if optional.multipart
                && proof.external_member_alias(member, AggregateFamily::Multipart, observer_may_alias)?
            {
                optional.multipart = false;
            }
        }
    }
    for program in captures {
        if !optional.hpp && !optional.multipart {
            break;
        }
        for index in 0..program.capture_len() {
            proof.record()?;
            if let Some(name) = program.capture_name(index) {
                if optional.hpp && proof.aggregate_key(name, AggregateFamily::Hpp, true)? {
                    optional.hpp = false;
                }
                if optional.multipart && proof.aggregate_key(name, AggregateFamily::Multipart, true)? {
                    optional.multipart = false;
                }
            }
        }
    }
    Ok(optional)
}

#[cfg(test)]
#[path = "../../tests/unit/seclang/expensive_checks_test.rs"]
mod tests;
