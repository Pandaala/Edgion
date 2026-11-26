//! Final publication of a built generation under the compile work budget.

use super::*;

impl CompiledGeneration {
    pub(super) fn publish_with_work(
        mut state: GroupedMatcherState<'_>,
        limits: CompilerLimits,
    ) -> Result<(Self, CanonicalWork), CompilerError> {
        let active_units = state.matchers.asset_templates.validated.active_rule_units().len();
        let marker_count = state.matchers.asset_templates.validated.markers().len();
        let mut phase_units = [0_usize; 5];
        for unit in state.matchers.asset_templates.validated.active_rule_units() {
            let phase = unit.root().resolved_state().phase();
            phase_units[phase_index(phase)] = checked_inc(phase_units[phase_index(phase)])?;
        }
        let marker_entries = marker_count.checked_mul(5).ok_or(CompilerError::PreflightArithmetic)?;
        let total_entries = active_units
            .checked_add(marker_entries)
            .ok_or(CompilerError::PreflightArithmetic)?;
        if total_entries > limits.max_schedule_entries {
            return Err(CompilerError::ScheduleEntryLimit {
                actual: total_entries,
                limit: limits.max_schedule_entries,
            });
        }
        let mut phase_entries = [0_usize; 5];
        for index in 0..5 {
            phase_entries[index] = phase_units[index]
                .checked_add(marker_count)
                .ok_or(CompilerError::PreflightArithmetic)?;
            if phase_entries[index] > limits.max_schedule_entries_per_phase {
                return Err(CompilerError::PhaseScheduleEntryLimit {
                    phase: index + 1,
                    actual: phase_entries[index],
                    limit: limits.max_schedule_entries_per_phase,
                });
            }
        }

        let mut work = MatcherCompileWork {
            limits,
            validated_bytes: state.matchers.asset_templates.compilation_input_bytes,
            working_bytes: state.matchers.asset_templates.compiler_working_bytes,
            compiler_owned_bytes: state.compiler_owned_bytes,
            matcher_charged_bytes: state.matcher_charged_bytes,
            peak_bytes: state.peak_bytes,
            requirement_cursors: [0; 8],
        };
        work.retain_working_records::<DirectiveLocation>(
            active_units
                .checked_add(marker_count)
                .ok_or(CompilerError::PreflightArithmetic)?,
        )?;
        work.retain_compiler_records::<CompiledRuleUnit>(active_units)?;
        work.retain_compiler_records::<CompiledMarker>(marker_count)?;
        work.retain_compiler_records::<CompiledScheduleEntry>(total_entries)?;

        let mut units = Vec::new();
        let mut unit_locations = Vec::new();
        let mut markers = Vec::new();
        let mut marker_locations = Vec::new();
        let mut schedules: [Vec<CompiledScheduleEntry>; 5] = std::array::from_fn(|_| Vec::new());
        reserve_exact(&mut units, active_units, "compiled rule units")?;
        reserve_exact(&mut unit_locations, active_units, "compiled rule locations")?;
        reserve_exact(&mut markers, marker_count, "compiled markers")?;
        reserve_exact(&mut marker_locations, marker_count, "compiled marker locations")?;
        for index in 0..5 {
            reserve_exact(&mut schedules[index], phase_entries[index], "compiled phase schedule")?;
        }

        let optional_checks = expensive_checks::classify(
            &state.matchers.asset_templates.validated,
            &state.matchers.capture_regexes,
            &mut state.canonical_work,
            limits,
        )?;
        let mut member_start = 0_usize;
        for unit in state.matchers.asset_templates.validated.rule_units() {
            let member_count = unit.members().len();
            if unit.is_active() {
                let root = unit.root();
                let resolved = root.resolved_state();
                unit_locations.push(root.rule().location());
                units.push(CompiledRuleUnit {
                    expensive_check: optional_checks.skips_unit(unit.id()),
                    requires_request_body: false,
                    id: unit.id(),
                    phase: resolved.phase(),
                    disruptive: resolved.disruptive(),
                    status: resolved.status(),
                    log: resolved.log(),
                    audit_log: resolved.audit_log(),
                    member_start,
                    member_count,
                    schedule_index: usize::MAX,
                });
            }
            member_start = checked_add(member_start, member_count)?;
        }
        for marker in state.matchers.asset_templates.validated.markers() {
            marker_locations.push(marker.location());
            markers.push(CompiledMarker {
                schedule_indices: [usize::MAX; 5],
            });
        }

        let mut unit_index = 0_usize;
        let mut marker_index = 0_usize;
        while unit_index < units.len() || marker_index < markers.len() {
            let take_unit = match (unit_locations.get(unit_index), marker_locations.get(marker_index)) {
                (Some(unit), Some(marker)) => unit < marker,
                (Some(_), None) => true,
                (None, Some(_)) => false,
                (None, None) => break,
            };
            if take_unit {
                let phase = phase_index(units[unit_index].phase);
                units[unit_index].schedule_index = schedules[phase].len();
                schedules[phase].push(CompiledScheduleEntry::RuleUnit(unit_index));
                unit_index = checked_inc(unit_index)?;
            } else {
                if unit_locations.get(unit_index) == marker_locations.get(marker_index) {
                    return Err(CompilerError::PrivateInvariant {
                        kind: CompilerInvariant::ScheduleOrder,
                    });
                }
                for (phase, schedule) in schedules.iter_mut().enumerate() {
                    markers[marker_index].schedule_indices[phase] = schedule.len();
                    schedule.push(CompiledScheduleEntry::MarkerSentinel(marker_index));
                }
                marker_index = checked_inc(marker_index)?;
            }
        }
        if schedules
            .iter()
            .zip(phase_entries)
            .any(|(schedule, expected)| schedule.len() != expected)
        {
            return Err(CompilerError::PrivateInvariant {
                kind: CompilerInvariant::ScheduleOrder,
            });
        }
        for unit in &units {
            let member_end = unit
                .member_start
                .checked_add(unit.member_count)
                .ok_or(CompilerError::PreflightArithmetic)?;
            for member in state
                .matchers
                .asset_templates
                .members
                .get_mut(unit.member_start..member_end)
                .ok_or(CompilerError::PrivateInvariant {
                    kind: CompilerInvariant::ScheduleOrder,
                })?
            {
                member.expensive_aggregate_actions = optional_checks.multipart && unit.id == 922150;
                let Some(marker) = member.skip_after_marker else {
                    continue;
                };
                let phase = phase_index(member.phase);
                let sentinel = markers
                    .get(marker)
                    .ok_or(CompilerError::PrivateInvariant {
                        kind: CompilerInvariant::ScheduleOrder,
                    })?
                    .schedule_indices[phase];
                if sentinel <= unit.schedule_index {
                    return Err(CompilerError::PrivateInvariant {
                        kind: CompilerInvariant::ScheduleOrder,
                    });
                }
                member.skip_after_marker = Some(sentinel);
            }
        }

        work.drop_working_records::<DirectiveLocation>(
            active_units
                .checked_add(marker_count)
                .ok_or(CompilerError::PreflightArithmetic)?,
        );
        let GroupedMatcherState {
            pattern_preflight,
            matchers,
            target_bytes,
            target_records,
            target_plans,
            canonical_transformations,
            transformation_plans,
            groups,
            pattern_mappings,
            capture_phrases,
            ..
        } = state;
        let MatcherState {
            asset_templates,
            operator_plans,
            ip_networks,
            target_regexes,
            mut boolean_regexes,
            capture_regexes,
            ..
        } = matchers;
        let AssetTemplateState {
            compilation_input_bytes: _,
            validated,
            bytes,
            templates,
            segments,
            macros,
            mut actions,
            setvars,
            members,
            ..
        } = asset_templates;
        let mut target_bytes = target_bytes;
        for action in &mut actions {
            let CompiledActionOperand::Ctl(CompiledCtl::RuleRemoveTargetByTag {
                collection, selector, ..
            }) = &mut action.operand
            else {
                continue;
            };
            let resolved = match *selector {
                CompiledCtlSelector::None => CompiledTargetSelector::None,
                CompiledCtlSelector::Literal(source) | CompiledCtlSelector::Path(source) => {
                    let value = byte_range(&bytes, source)?;
                    if matches!(*selector, CompiledCtlSelector::Path(_)) && !matches!(value, b"/*" | b"//@*") {
                        return Err(CompilerError::CtlXmlSelector {
                            location: action.location,
                        });
                    }
                    let range = ByteRange {
                        start: target_bytes.len(),
                        len: value.len(),
                    };
                    work.reserve_canonical(&mut target_bytes, value.len(), "compiled ctl target bytes")?;
                    target_bytes.extend_from_slice(value);
                    if matches!(*selector, CompiledCtlSelector::Literal(_)) {
                        CompiledTargetSelector::Literal(range)
                    } else {
                        CompiledTargetSelector::Path(range)
                    }
                }
                CompiledCtlSelector::RegexSource(source) => {
                    let count = boolean_regexes
                        .len()
                        .checked_add(capture_regexes.len())
                        .and_then(|count| count.checked_add(1))
                        .ok_or(CompilerError::PreflightArithmetic)?;
                    if count > limits.max_regex_programs {
                        return Err(CompilerError::RegexProgramLimit {
                            actual: count,
                            limit: limits.max_regex_programs,
                        });
                    }
                    let pattern = std::str::from_utf8(byte_range(&bytes, source)?).map_err(|_| {
                        CompilerError::PrivateInvariant {
                            kind: CompilerInvariant::InvalidByteRange,
                        }
                    })?;
                    let regex = if *collection == CollectionKind::Tx {
                        work.compile_ascii_case_insensitive_boolean_regex(pattern, action.location, true)?
                    } else {
                        work.compile_boolean_regex(pattern, action.location, true)?
                    };
                    work.retain_compiler_records::<CompiledRegexProgram>(1)?;
                    boolean_regexes.try_reserve(1).map_err(|_| CompilerError::Allocation {
                        operation: "compiled ctl target regex",
                    })?;
                    let program = boolean_regexes.len();
                    boolean_regexes.push(regex);
                    CompiledTargetSelector::Regex {
                        style: RegexStyle::Bare,
                        program,
                    }
                }
                CompiledCtlSelector::Resolved(_) => {
                    return Err(CompilerError::PrivateInvariant {
                        kind: CompilerInvariant::UnexpectedActionOperand,
                    });
                }
            };
            *selector = CompiledCtlSelector::Resolved(resolved);
        }
        let tag_count = units.iter().try_fold(0_usize, |mut total, unit| {
            let end = unit
                .member_start
                .checked_add(unit.member_count)
                .ok_or(CompilerError::PreflightArithmetic)?;
            let members = members
                .get(unit.member_start..end)
                .ok_or(CompilerError::PrivateInvariant {
                    kind: CompilerInvariant::ScheduleOrder,
                })?;
            for member in members {
                let end = member
                    .action_start
                    .checked_add(member.action_count)
                    .ok_or(CompilerError::PreflightArithmetic)?;
                let actions = actions
                    .get(member.action_start..end)
                    .ok_or(CompilerError::PrivateInvariant {
                        kind: CompilerInvariant::ScheduleOrder,
                    })?;
                total = total
                    .checked_add(
                        actions
                            .iter()
                            .filter(|action| {
                                matches!(
                                    (action.kind, action.operand),
                                    (
                                        ActionEnvelopeKind::Action(ActionKind::Tag),
                                        CompiledActionOperand::Literal(_)
                                    )
                                )
                            })
                            .count(),
                    )
                    .ok_or(CompilerError::PreflightArithmetic)?;
            }
            Ok::<_, CompilerError>(total)
        })?;
        work.retain_compiler_records::<CompiledTagRuleRef>(tag_count)?;
        let mut tag_rule_refs = Vec::new();
        reserve_exact(&mut tag_rule_refs, tag_count, "compiled ctl tag index")?;
        for (unit_id, unit) in units.iter().enumerate() {
            for member in &members[unit.member_start..unit.member_start + unit.member_count] {
                for action in &actions[member.action_start..member.action_start + member.action_count] {
                    if let (ActionEnvelopeKind::Action(ActionKind::Tag), CompiledActionOperand::Literal(tag)) =
                        (action.kind, action.operand)
                    {
                        tag_rule_refs.push(CompiledTagRuleRef { tag, unit: unit_id });
                    }
                }
            }
        }
        tag_rule_refs.sort_unstable_by(|left, right| {
            byte_range(&bytes, left.tag)
                .expect("compiled tag range is valid")
                .cmp(byte_range(&bytes, right.tag).expect("compiled tag range is valid"))
                .then(left.unit.cmp(&right.unit))
        });
        let mut generation = Self {
            capture_hits: capture_hit_plan::CaptureHitPlan::default(),
            inspection_activation: inspection_activation::CompiledActivation::default(),
            json_inspection: json_inspection_plan::CompiledJsonPlan::default(),
            xml_inspection: json_inspection_plan::CompiledJsonPlan::default(),
            has_expensive_checks: optional_checks.hpp || optional_checks.multipart,
            prefix_validation: Ok(()),
            requires_complete_request_body: false,
            reads_body_error: false,
            bytes,
            templates,
            segments,
            macros,
            actions,
            tag_rule_refs,
            setvars,
            members,
            operator_plans,
            ip_networks,
            target_regexes,
            boolean_regexes,
            capture_regexes,
            target_bytes,
            target_records,
            target_plans,
            transformations: canonical_transformations,
            transformation_plans,
            groups,
            pattern_mappings,
            capture_phrases,
            units,
            markers,
            schedules,
            compiler_owned_bytes: work.compiler_owned_bytes,
            matcher_charged_bytes: work.matcher_charged_bytes,
            peak_bytes: work.peak_bytes,
        };
        generation.classify_request_body_dependencies(&mut state.canonical_work, limits)?;
        generation.prefix_validation = generation.check_request_body_prefix();
        generation.inspection_activation =
            inspection_activation::CompiledActivation::compile(&generation, &mut work, &mut state.canonical_work)?;
        generation.json_inspection =
            json_inspection_plan::CompiledJsonPlan::compile(&generation, &mut work, &mut state.canonical_work)?;
        generation.xml_inspection = json_inspection_plan::CompiledJsonPlan::compile_domain(
            &generation,
            &mut work,
            &mut state.canonical_work,
            InspectionDomain::Xml,
        )?;
        capture_hit_plan::compile_optional(
            &mut generation,
            &validated,
            pattern_preflight,
            &mut work,
            &mut state.canonical_work,
        )?;
        for program in &mut generation.boolean_regexes {
            install_optional_regex_hint(
                program,
                limits.regex.max_program_bytes,
                &mut work,
                &mut state.canonical_work,
            )?;
        }
        for matcher in generation
            .groups
            .iter_mut()
            .map(|group| &mut group.matcher)
            .chain(generation.capture_hits.matchers.iter_mut())
        {
            if let Some(program) = matcher.regex_program_mut() {
                install_optional_regex_hint(
                    program,
                    limits.matcher.max_regex_program_bytes,
                    &mut work,
                    &mut state.canonical_work,
                )?;
            }
        }
        // Capture programs decide matches through their capture-free decision
        // program; hint it last so earlier hints keep their budgets.
        for program in &mut generation.capture_regexes {
            let (decision, program_cap) = program.decision_hint_target(limits.regex.max_program_bytes);
            install_optional_regex_hint(decision, program_cap, &mut work, &mut state.canonical_work)?;
        }
        generation.compiler_owned_bytes = work.compiler_owned_bytes;
        generation.matcher_charged_bytes = work.matcher_charged_bytes;
        generation.peak_bytes = work.peak_bytes;
        Ok((generation, state.canonical_work))
    }
}
