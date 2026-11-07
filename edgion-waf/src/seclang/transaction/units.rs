use super::*;

impl<'generation> Transaction<'generation> {
    pub(super) fn evaluate_unit(&mut self, unit_id: usize, prefix_match: Option<bool>) -> Result<bool, RuntimeError> {
        self.unit_work_bytes = 0;
        // Keep capacity charged while the buffer is loaned to the entire unit.
        let mut matches = std::mem::take(&mut self.match_scratch);
        matches.clear();
        let result = self.evaluate_unit_into(unit_id, prefix_match, &mut matches);
        matches.clear();
        self.match_scratch = matches;
        result
    }

    fn evaluate_unit_into(
        &mut self,
        unit_id: usize,
        prefix_match: Option<bool>,
        matches: &mut Vec<MemberMatch>,
    ) -> Result<bool, RuntimeError> {
        let unit = *self
            .generation
            .units
            .get(unit_id)
            .ok_or(RuntimeError::GenerationInvariant)?;
        let end = unit
            .member_start
            .checked_add(unit.member_count)
            .ok_or(RuntimeError::Arithmetic)?;
        self.evaluate_unit_first_member(
            unit_id,
            unit.member_start,
            if unit.member_count == 1 { prefix_match } else { None },
            matches,
        )?;
        if matches.is_empty() {
            return Ok(false);
        }
        // Stage the unit in place instead of copying the TX map: the base state
        // moves into `staged` and a placeholder keeping the base retained charge
        // stands in for it, exactly as the base stayed charged beside the former
        // per-unit copy. Edits are journaled; a unit that does not commit (no
        // match or any error) rolls back to the identical base state.
        let mut staged = std::mem::take(&mut self.tx);
        self.tx.retained_bytes = staged.retained_bytes;
        let checkpoint = staged.begin_unit();
        let mut journal = std::mem::take(&mut self.tx_journal);
        // A copied chain branch commits as `Some(tx)`; a single-member unit
        // commits the in-place staged state itself.
        let result = if unit.member_count == 1 {
            self.stage_single_member_unit(unit, &mut staged, &mut journal, matches)
                .map(|capture_keys| Some((None, capture_keys)))
        } else {
            self.stage_chain_unit(unit_id, unit, end, &staged, matches)
                .map(|winner| winner.map(|branch| (Some(branch.tx), branch.capture_keys)))
        };
        let committed = match result {
            Ok(Some((branch, capture_keys))) => {
                self.tx = branch.unwrap_or(staged);
                self.capture_keys = capture_keys;
                Ok(true)
            }
            Ok(None) => {
                staged.rollback_unit(&mut journal, checkpoint);
                self.tx = staged;
                Ok(false)
            }
            Err(error) => {
                staged.rollback_unit(&mut journal, checkpoint);
                self.tx = staged;
                Err(error)
            }
        };
        journal.clear();
        self.tx_journal = journal;
        committed
    }

    /// Apply every first-member match and its actions to the in-place staged
    /// unit, journaling each edit, then admit the commit.
    fn stage_single_member_unit(
        &mut self,
        unit: CompiledRuleUnit,
        staged: &mut TxState,
        journal: &mut TxJournal,
        matches: &mut Vec<MemberMatch>,
    ) -> Result<Vec<MatchValue>, RuntimeError> {
        let mut capture_keys = try_clone_keys(&self.capture_keys)?;
        let mut setvar_actions = 0_usize;
        for matched in matches.drain(..) {
            apply_member_match(
                staged,
                &mut capture_keys,
                matched,
                self.limits,
                &self.request,
                self.json_budget.as_mut(),
                Some(&mut *journal),
            )?;
            self.apply_member_actions(unit.member_start, staged, &mut setvar_actions, Some(&mut *journal))?;
        }
        self.admit_unit_commit(unit, staged)?;
        Ok(capture_keys)
    }

    /// Evaluate a chain over a frontier of branch copies. The in-place staged
    /// unit is the single seed branch: it is only read and copied into
    /// children, never edited, and it is charged as one frontier branch.
    fn stage_chain_unit(
        &mut self,
        unit_id: usize,
        unit: CompiledRuleUnit,
        end: usize,
        staged: &TxState,
        matches: &mut Vec<MemberMatch>,
    ) -> Result<Option<ChainBranch>, RuntimeError> {
        let seed_bytes = size_of::<ChainBranch>()
            .checked_add(staged_branch_bytes(staged, &self.capture_keys)?)
            .ok_or(RuntimeError::Arithmetic)?;
        let mut frontier = Vec::new();
        let mut next_bytes = 0_usize;
        for matched in matches.drain(..) {
            let capture_keys = try_clone_keys(&self.capture_keys)?;
            let candidate = self.chain_child(unit.member_start, staged.try_clone()?, capture_keys, 0, matched)?;
            self.admit_chain_child(1, seed_bytes, &mut frontier, &mut next_bytes, candidate)?;
        }
        if frontier.is_empty() {
            return Ok(None);
        }
        for member_id in unit.member_start + 1..end {
            let final_member = member_id + 1 == end;
            let current_count = frontier.len();
            let current_bytes = chain_branches_bytes(&frontier)?;
            if final_member {
                // Only staged branches are consumed. Preserve the original
                // frontier checkpoint before selecting children in winner order.
                for mut branch in frontier {
                    self.evaluate_member_matches_into(unit_id, member_id, &branch.tx, true, matches)?;
                    if let Some(matched) = matches.drain(..).next() {
                        apply_member_match(
                            &mut branch.tx,
                            &mut branch.capture_keys,
                            matched,
                            self.limits,
                            &self.request,
                            self.json_budget.as_mut(),
                            None,
                        )?;
                        self.apply_member_actions(member_id, &mut branch.tx, &mut branch.setvar_actions, None)?;
                        self.admit_unit_commit(unit, &branch.tx)?;
                        return Ok(Some(branch));
                    }
                }
                return Ok(None);
            }
            let mut next = Vec::new();
            let mut next_bytes = 0_usize;
            for branch in &frontier {
                self.evaluate_member_matches_into(unit_id, member_id, &branch.tx, final_member, matches)?;
                for matched in matches.drain(..) {
                    let candidate = self.chain_child(
                        member_id,
                        branch.tx.try_clone()?,
                        try_clone_keys(&branch.capture_keys)?,
                        branch.setvar_actions,
                        matched,
                    )?;
                    self.admit_chain_child(current_count, current_bytes, &mut next, &mut next_bytes, candidate)?;
                }
            }
            if next.is_empty() {
                return Ok(None);
            }
            frontier = next;
        }
        Ok(None)
    }

    /// Apply one member match and its actions to a copied frontier child.
    fn chain_child(
        &mut self,
        member_id: usize,
        tx: TxState,
        capture_keys: Vec<MatchValue>,
        setvar_actions: usize,
        matched: MemberMatch,
    ) -> Result<ChainBranch, RuntimeError> {
        let mut candidate = ChainBranch {
            tx,
            capture_keys,
            setvar_actions,
        };
        apply_member_match(
            &mut candidate.tx,
            &mut candidate.capture_keys,
            matched,
            self.limits,
            &self.request,
            self.json_budget.as_mut(),
            None,
        )?;
        self.apply_member_actions(member_id, &mut candidate.tx, &mut candidate.setvar_actions, None)?;
        Ok(candidate)
    }

    /// Bound the next frontier by live branch count and branch bytes.
    fn admit_chain_child(
        &self,
        current_count: usize,
        current_bytes: usize,
        next: &mut Vec<ChainBranch>,
        next_bytes: &mut usize,
        candidate: ChainBranch,
    ) -> Result<(), RuntimeError> {
        if current_count.saturating_add(next.len()) >= self.limits.max_operator_evaluations {
            return Err(RuntimeError::OperatorEvaluationLimit {
                limit: self.limits.max_operator_evaluations,
            });
        }
        *next_bytes = next_bytes
            .checked_add(chain_branch_bytes(&candidate)?)
            .ok_or(RuntimeError::Arithmetic)?;
        if current_bytes.checked_add(*next_bytes).ok_or(RuntimeError::Arithmetic)? > self.limits.max_operator_work_bytes
        {
            return Err(RuntimeError::OperatorWorkLimit {
                limit: self.limits.max_operator_work_bytes,
            });
        }
        next.try_reserve(1).map_err(|_| RuntimeError::Allocation)?;
        next.push(candidate);
        Ok(())
    }

    /// Loan persistent TX storage while hiding prior-unit match observers.
    /// The accounting placeholder preserves original retained admission. Restore
    /// ownership before propagating any evaluation result or staging actions.
    pub(super) fn evaluate_unit_first_member(
        &mut self,
        unit_id: usize,
        member_id: usize,
        prefix_match: Option<bool>,
        matches: &mut Vec<MemberMatch>,
    ) -> Result<(), RuntimeError> {
        let mut base = std::mem::take(&mut self.tx);
        self.tx.retained_bytes = base.retained_bytes;
        let previous = (
            base.matched_var.take(),
            base.matched_var_name.take(),
            std::mem::take(&mut base.matched_vars),
            base.matched_vars_payload_bytes,
        );
        base.matched_vars_payload_bytes = 0;
        base.refresh_retained_bytes();
        let result = match prefix_match {
            Some(matched) => self.boolean_member_matches_into(matched, &base, matches),
            None => self.evaluate_member_matches_into(unit_id, member_id, &base, false, matches),
        };
        base.matched_var = previous.0;
        base.matched_var_name = previous.1;
        base.matched_vars = previous.2;
        base.matched_vars_payload_bytes = previous.3;
        base.refresh_retained_bytes();
        self.tx = base;
        result
    }

    /// Validate runtime-expanded actions and the retained charge of the state
    /// about to replace the base TX. Nothing is published on error.
    pub(super) fn admit_unit_commit(&mut self, unit: CompiledRuleUnit, staged: &TxState) -> Result<(), RuntimeError> {
        self.validate_runtime_actions(unit, staged)?;
        let total_retained = self
            .request
            .retained_bytes
            .checked_add(self.scratch_bytes)
            .and_then(|value| value.checked_add(staged.retained_bytes))
            .and_then(|value| value.checked_add(self.response.as_ref().map_or(0, ResponseState::total_retained_bytes)))
            .ok_or(RuntimeError::Arithmetic)?;
        if total_retained > self.limits.max_retained_bytes {
            return Err(RuntimeError::RetainedByteLimit {
                limit: self.limits.max_retained_bytes,
            });
        }
        Ok(())
    }

    fn boolean_member_matches_into(
        &mut self,
        matched: bool,
        tx: &TxState,
        matches: &mut Vec<MemberMatch>,
    ) -> Result<(), RuntimeError> {
        if matched {
            self.match_storage(tx, 1)?.push(
                matches,
                MemberMatch::default(),
                &mut self.scratch_bytes,
                &mut self.match_scratch_bytes,
            )?;
        }
        Ok(())
    }

    fn match_storage(&self, tx: &TxState, limit: usize) -> Result<MatchStorage, RuntimeError> {
        Ok(MatchStorage {
            fixed_bytes: self
                .retained_bytes()
                .checked_sub(self.match_scratch_bytes)
                .and_then(|bytes| bytes.checked_add(tx.retained_bytes.saturating_sub(self.tx.retained_bytes)))
                .ok_or(RuntimeError::Arithmetic)?,
            retained_limit: self.limits.max_retained_bytes,
            limit,
        })
    }

    #[cfg(test)]
    pub(super) fn evaluate_member_matches(
        &mut self,
        unit_id: usize,
        member_id: usize,
        tx: &TxState,
        stop_after_first: bool,
    ) -> Result<Vec<MemberMatch>, RuntimeError> {
        let mut matches = std::mem::take(&mut self.match_scratch);
        matches.clear();
        // Each direct call models one member invocation of a fresh unit.
        self.unit_work_bytes = 0;
        let result = self.evaluate_member_matches_into(unit_id, member_id, tx, stop_after_first, &mut matches);
        // Private tests may inspect the returned matches. Production always
        // drains and returns capacity through evaluate_unit's single loan.
        self.scratch_bytes = self
            .scratch_bytes
            .checked_sub(self.match_scratch_bytes)
            .ok_or(RuntimeError::Arithmetic)?;
        self.match_scratch_bytes = 0;
        result.map(|()| matches)
    }

    pub(super) fn evaluate_member_matches_into(
        &mut self,
        unit_id: usize,
        member_id: usize,
        tx: &TxState,
        stop_after_first: bool,
        matches: &mut Vec<MemberMatch>,
    ) -> Result<(), RuntimeError> {
        matches.clear();
        if let Some(matched) = self
            .stream_member_matches
            .as_ref()
            .and_then(|members| members.get(member_id))
            .copied()
            .flatten()
        {
            return self.boolean_member_matches_into(matched, tx, matches);
        }
        let member = *self
            .generation
            .members
            .get(member_id)
            .ok_or(RuntimeError::GenerationInvariant)?;
        let Some(operator) = self.generation.operator_plans.get(member_id).copied().flatten() else {
            return self.boolean_member_matches_into(true, tx, matches);
        };
        let target_plan = member.target_plan.ok_or(RuntimeError::GenerationInvariant)?;
        let old_capacity_bytes = self
            .selected_scratch
            .capacity()
            .checked_mul(size_of::<SelectedValue>())
            .ok_or(RuntimeError::Arithmetic)?;
        let other_scratch_bytes = self
            .scratch_bytes
            .checked_sub(old_capacity_bytes)
            .ok_or(RuntimeError::Arithmetic)?;
        let fixed_bytes = self
            .retained_bytes()
            .checked_sub(old_capacity_bytes)
            .and_then(|bytes| bytes.checked_add(tx.retained_bytes.saturating_sub(self.tx.retained_bytes)))
            .ok_or(RuntimeError::Arithmetic)?;
        let capacity_bytes =
            self.limits
                .max_retained_bytes
                .checked_sub(fixed_bytes)
                .ok_or(RuntimeError::RetainedByteLimit {
                    limit: self.limits.max_retained_bytes,
                })?;
        if old_capacity_bytes > capacity_bytes {
            return Err(RuntimeError::RetainedByteLimit {
                limit: self.limits.max_retained_bytes,
            });
        }
        let mut json_tail = self.json_summary_no_hit_tail(member_id, tx)?;
        let mut selected = std::mem::take(&mut self.selected_scratch);
        selected.clear();
        let mut work = self.unit_work_bytes;
        let result = (|| {
            let storage = SelectionStorage {
                capacity_bytes,
                retained_limit: self.limits.max_retained_bytes,
            };
            if json_tail.is_some_and(|tail| {
                storage
                    .summary_capacity(
                        selected.capacity(),
                        tail.selected_count,
                        self.limits.max_selected_values,
                    )
                    .is_none()
            }) {
                json_tail = None;
            }
            let mut summary_match_storage = None;
            loop {
                let selection_result = select_values(
                    &mut SelectionCtx {
                        generation: &self.generation,
                        request: &self.request,
                        tx,
                        response: self.response.as_ref(),
                        scratch: &mut self.boolean_scratch,
                        json_budget: self.json_budget.as_mut(),
                        #[cfg(test)]
                        generic_selection: self.force_generic_selection,
                    },
                    &mut self.ctl_work_bytes,
                    SelectionParams {
                        unit_id,
                        plan_id: target_plan,
                        limit: self.limits.max_selected_values,
                        work_limit: self.limits.max_operator_work_bytes,
                        include_json: json_tail.is_none(),
                    },
                    &mut selected,
                    storage,
                );
                let actual_bytes = selected
                    .capacity()
                    .checked_mul(size_of::<SelectedValue>())
                    .ok_or(RuntimeError::Arithmetic)?;
                self.scratch_bytes = other_scratch_bytes
                    .checked_add(actual_bytes)
                    .ok_or(RuntimeError::Arithmetic)?;
                selection_result?;
                let Some(tail) = json_tail else {
                    break;
                };
                let hypothetical = storage.summary_capacity(
                    selected.capacity(),
                    tail.selected_count,
                    self.limits.max_selected_values,
                );
                if let Some(hypothetical_bytes) = hypothetical.filter(|_| selected.len() == tail.query_count) {
                    let limit = if stop_after_first { 1 } else { tail.selected_count };
                    let mut admission = self.match_storage(tx, limit)?;
                    if let Some(fixed_bytes) = hypothetical_bytes
                        .checked_sub(actual_bytes)
                        .and_then(|extra| admission.fixed_bytes.checked_add(extra))
                    {
                        admission.fixed_bytes = fixed_bytes;
                        let query_matches = if stop_after_first {
                            tail.query_count.min(1)
                        } else {
                            tail.query_count
                        };
                        if admission.admits_growth(matches.capacity(), query_matches) {
                            // Only match admission sees the virtual tail capacity.
                            // Persistent scratch always records actual allocations.
                            summary_match_storage = Some(admission);
                            self.json_budget
                                .as_mut()
                                .ok_or(RuntimeError::JsonInspectionState)?
                                .work(tail.selection_work)
                                .map_err(map_store_error)?;
                            break;
                        }
                    }
                }
                // Query-only selection has no physical or control work under
                // this static singleton proof. Re-select before the first charge
                // if actual allocation or hypothetical match admission is unsure.
                json_tail = None;
                selected.clear();
            }
            let transformations = member
                .transformation_plan
                .and_then(|id| self.generation.transformation_plans.get(id))
                .and_then(|plan| {
                    let end = plan.transformation_start.checked_add(plan.transformation_count)?;
                    self.generation.transformations.get(plan.transformation_start..end)
                })
                .unwrap_or_default();
            let multi_match = member_has_action(&self.generation, member, ActionKind::MultiMatch)?;
            // The checked nonempty slice above authenticates the canonical plan ID.
            let memo_plan_id =
                if self.evaluation_mode == EvaluationMode::Accelerated && !multi_match && !transformations.is_empty() {
                    member.transformation_plan
                } else {
                    None
                };
            let match_limit = if stop_after_first { 1 } else { selected.len() };
            let match_storage = match summary_match_storage {
                Some(storage) => storage,
                None => self.match_storage(tx, match_limit)?,
            };
            for selected in selected.iter().copied() {
                let transform_memo =
                    self.borrowed_transform_memo
                        .prepare(selected.value, memo_plan_id, &self.request, self.limits);
                let scan_cache = self.ordinary_scan_cache.prepare(
                    selected.value,
                    member.transformation_plan,
                    !multi_match
                        && self.evaluation_mode == EvaluationMode::Accelerated
                        && matches!(operator.operand, CompiledOperatorOperand::Matcher { .. }),
                );
                let mut context = SingleInputContext {
                    generation: &self.generation,
                    request: &self.request,
                    tx,
                    group_scratch: &mut self.group_scratch,
                    boolean_scratch: &mut self.boolean_scratch,
                    capture_scratch: &mut self.capture_scratch,
                    scan_cache,
                    json_budget: self.json_budget.as_mut(),
                    transform_memo: None,
                };
                let matched = if let SelectedBytes::Json(id)
                | SelectedBytes::XmlText(id)
                | SelectedBytes::XmlAttribute(id) = selected.value
                {
                    json_inspection_runtime::evaluate_json_selected(
                        &mut context,
                        SingleInputPlan {
                            operator,
                            transformations,
                            multi_match,
                            runtime_expansion_byte_limit: member.runtime_expansion_byte_limit,
                            limits: self.limits,
                            evaluation_mode: self.evaluation_mode,
                        },
                        member_id,
                        id,
                        selected.collection,
                        &mut OrderedStageAccounting::new(self.limits, &mut work, &mut self.detector_work_bytes),
                        matches.len(),
                    )
                    .map_err(|error| {
                        if selected.collection == CollectionKind::Xml {
                            xml_inspection_runtime::xml_error(error)
                        } else {
                            error
                        }
                    })?
                } else {
                    let value = selected.bytes(
                        &self.request,
                        tx,
                        self.response.as_ref(),
                        context.json_budget.as_deref_mut(),
                    )?;
                    // Every stage charge includes the original bytes; reject
                    // before transforming once the unit cannot afford them.
                    if work.checked_add(value.len()).ok_or(RuntimeError::Arithmetic)?
                        > self.limits.max_operator_work_bytes
                    {
                        return Err(RuntimeError::OperatorWorkLimit {
                            limit: self.limits.max_operator_work_bytes,
                        });
                    }
                    context.transform_memo = transform_memo;
                    evaluate_single_input(
                        &mut context,
                        SingleInputPlan {
                            operator,
                            transformations,
                            multi_match,
                            runtime_expansion_byte_limit: member.runtime_expansion_byte_limit,
                            limits: self.limits,
                            evaluation_mode: self.evaluation_mode,
                        },
                        value,
                        &mut OrderedStageAccounting::new(self.limits, &mut work, &mut self.detector_work_bytes),
                        matches.len(),
                        |transformed, budget| {
                            matched_variable(&self.request, tx, self.response.as_ref(), selected, transformed, budget)
                        },
                    )?
                };
                if let Some(matched) = matched {
                    match_storage.push(matches, matched, &mut self.scratch_bytes, &mut self.match_scratch_bytes)?;
                    if stop_after_first {
                        return Ok(());
                    }
                }
            }
            if let Some(tail) = json_tail {
                // Combined logical and physical admission was proven before
                // selection. Query errors/early stops never reach this replay.
                charge_operator_work(&mut work, tail.logical_work, self.limits)?;
                self.json_budget
                    .as_mut()
                    .ok_or(RuntimeError::JsonInspectionState)?
                    .work(tail.replay_work)
                    .map_err(map_store_error)?;
            }
            Ok(())
        })();
        self.unit_work_bytes = work;
        selected.clear();
        let actual_bytes = selected
            .capacity()
            .checked_mul(size_of::<SelectedValue>())
            .ok_or(RuntimeError::Arithmetic)?;
        if actual_bytes > capacity_bytes {
            self.scratch_bytes = self
                .scratch_bytes
                .checked_sub(actual_bytes)
                .ok_or(RuntimeError::Arithmetic)?;
            return Err(RuntimeError::RetainedByteLimit {
                limit: self.limits.max_retained_bytes,
            });
        }
        // Selection was charged before matching; preserve any match-buffer
        // growth recorded while selected_scratch was temporarily loaned.
        self.selected_scratch = selected;
        result
    }

    /// Prove a no-hit JSON tail without selecting or evaluating queries. Any
    /// uncertain bound preserves the cursor's selection-before-evaluation errors.
    pub(super) fn json_summary_no_hit_tail(
        &mut self,
        member_id: usize,
        tx: &TxState,
    ) -> Result<Option<JsonNoHitTail>, RuntimeError> {
        #[cfg(test)]
        if self.force_json_cursor {
            return Ok(None);
        }
        if self.evaluation_mode != EvaluationMode::Accelerated
            || !tx.target_exclusions.is_empty()
            || !self
                .generation
                .json_inspection
                .members
                .get(member_id)
                .is_some_and(|member| member.summary_no_hit)
        {
            return Ok(None);
        }
        let Some(results) = self
            .request
            .body
            .as_ref()
            .and_then(RequestBodyPublication::json)
            .and_then(|body| body.results.as_ref())
        else {
            return Ok(None);
        };
        if let JsonMemberMode::StaticFact(group) = self
            .generation
            .json_inspection
            .members
            .get(member_id)
            .ok_or(RuntimeError::GenerationInvariant)?
            .value
        {
            results
                .guard_group(
                    self.json_budget.as_mut().ok_or(RuntimeError::JsonInspectionState)?,
                    group,
                )
                .map_err(map_store_error)?;
        }
        let queries = self.request.collection_records(CollectionKind::Args)?;
        let bounds = (|| {
            let count = results.json_summary().ok()?.argument_count;
            let selected_count = queries.len().checked_add(count)?;
            let selection_work = count;
            let replay_work = count.checked_mul(3)?;
            let physical_work = selection_work.checked_add(replay_work)?;
            let logical_work = results.json_summary().ok()?.argument_value_bytes.checked_mul(2)?;
            let mut combined_work = logical_work;
            for query in queries {
                // None/lowercase preserve byte length. Charge a possible hit's
                // payload, qualified name and descriptor even for query misses.
                let name_len = CollectionKind::Args.as_str().len().checked_add(match query.name {
                    Some(name) => name.len.checked_add(1)?,
                    None => 0,
                })?;
                let query_work = query
                    .value
                    .len
                    .checked_mul(3)?
                    .checked_add(name_len)?
                    .checked_add(size_of::<MemberMatch>())?;
                combined_work = combined_work.checked_add(query_work)?;
            }
            Some((
                JsonNoHitTail {
                    query_count: queries.len(),
                    selected_count,
                    selection_work,
                    replay_work,
                    logical_work,
                },
                combined_work,
                physical_work,
            ))
        })();
        let Some((tail, combined_work, physical_work)) = bounds else {
            return Ok(None);
        };
        let Some(budget) = self.json_budget.as_ref() else {
            return Ok(None);
        };
        if tail.selected_count > self.limits.max_selected_values
            || combined_work
                .checked_add(self.unit_work_bytes)
                .is_none_or(|work| work > self.limits.max_operator_work_bytes)
            || !results.admits_summary_no_hit(
                budget,
                &self.generation,
                self.limits,
                self.evaluation_mode,
                physical_work,
            )
        {
            return Ok(None);
        }
        Ok(Some(tail))
    }

    fn validate_runtime_actions(&mut self, unit: CompiledRuleUnit, tx: &TxState) -> Result<(), RuntimeError> {
        let end = unit
            .member_start
            .checked_add(unit.member_count)
            .ok_or(RuntimeError::Arithmetic)?;
        for member in self
            .generation
            .members
            .get(unit.member_start..end)
            .ok_or(RuntimeError::GenerationInvariant)?
        {
            let action_end = member
                .action_start
                .checked_add(member.action_count)
                .ok_or(RuntimeError::Arithmetic)?;
            for action in self
                .generation
                .actions
                .get(member.action_start..action_end)
                .ok_or(RuntimeError::GenerationInvariant)?
            {
                if matches!(
                    action.kind,
                    ActionEnvelopeKind::Action(ActionKind::LogData | ActionKind::Msg)
                ) {
                    let CompiledActionOperand::Template(template) = action.operand else {
                        return Err(RuntimeError::GenerationInvariant);
                    };
                    let _expanded = expand_template::<false>(
                        &self.generation,
                        &self.request,
                        tx,
                        template,
                        member.runtime_expansion_byte_limit,
                        self.json_budget.as_mut(),
                    )?;
                }
            }
        }
        Ok(())
    }

    pub(super) fn finding_metadata(&self, unit: CompiledRuleUnit) -> Result<(Option<Severity>, usize), RuntimeError> {
        let end = unit
            .member_start
            .checked_add(unit.member_count)
            .ok_or(RuntimeError::Arithmetic)?;
        let mut severity = None;
        let mut tag_count = 0_usize;
        for member in self
            .generation
            .members
            .get(unit.member_start..end)
            .ok_or(RuntimeError::GenerationInvariant)?
        {
            let action_end = member
                .action_start
                .checked_add(member.action_count)
                .ok_or(RuntimeError::Arithmetic)?;
            for action in self
                .generation
                .actions
                .get(member.action_start..action_end)
                .ok_or(RuntimeError::GenerationInvariant)?
            {
                match (action.kind, action.operand) {
                    (
                        ActionEnvelopeKind::Action(ActionKind::Severity),
                        CompiledActionOperand::Local(LocalActionValue::Severity(value)),
                    ) => severity = Some(value),
                    (ActionEnvelopeKind::Action(ActionKind::Tag), CompiledActionOperand::Literal(_)) => {
                        tag_count = tag_count.checked_add(1).ok_or(RuntimeError::Arithmetic)?
                    }
                    _ => {}
                }
            }
        }
        Ok((severity, tag_count))
    }

    pub(super) fn next_schedule_ordinal(
        &self,
        unit: CompiledRuleUnit,
        schedule_index: usize,
        current: usize,
    ) -> Result<usize, RuntimeError> {
        inspection_activation::successor(
            &self.generation,
            unit,
            schedule_index,
            current,
            |_| Ok(()),
            || RuntimeError::GenerationInvariant,
        )
    }

    pub(super) fn apply_member_actions(
        &mut self,
        member_id: usize,
        staged: &mut TxState,
        setvar_actions: &mut usize,
        mut journal: Option<&mut TxJournal>,
    ) -> Result<(), RuntimeError> {
        let member = *self
            .generation
            .members
            .get(member_id)
            .ok_or(RuntimeError::GenerationInvariant)?;
        // The exact collector keeps its capture/observer path; only its two
        // aggregate setvar actions are optional, before expansion or allocation.
        if member.expensive_aggregate_actions && !staged.check_policy.expensive_checks {
            return Ok(());
        }
        let action_end = member
            .action_start
            .checked_add(member.action_count)
            .ok_or(RuntimeError::Arithmetic)?;
        for action_index in member.action_start..action_end {
            let action = *self
                .generation
                .actions
                .get(action_index)
                .ok_or(RuntimeError::GenerationInvariant)?;
            if let CompiledActionOperand::Ctl(ctl) = action.operand {
                self.apply_ctl(ctl, staged, journal.as_deref_mut())?;
                continue;
            }
            let CompiledActionOperand::SetVar(setvar_id) = action.operand else {
                continue;
            };
            *setvar_actions = (*setvar_actions).checked_add(1).ok_or(RuntimeError::Arithmetic)?;
            if *setvar_actions > self.limits.max_setvar_actions {
                return Err(RuntimeError::SetVarActionLimit {
                    limit: self.limits.max_setvar_actions,
                });
            }
            let setvar = *self
                .generation
                .setvars
                .get(setvar_id)
                .ok_or(RuntimeError::GenerationInvariant)?;
            if setvar.collection != CollectionKind::Tx {
                return Err(RuntimeError::GenerationInvariant);
            }
            let key = if setvar.operation == SetVarOperation::Delete {
                template_operand(
                    &self.generation,
                    &self.request,
                    staged,
                    setvar.key,
                    member.runtime_expansion_byte_limit,
                    self.json_budget.as_mut(),
                )?
            } else {
                Cow::Owned(expand_template::<true>(
                    &self.generation,
                    &self.request,
                    staged,
                    setvar.key,
                    member.runtime_expansion_byte_limit,
                    self.json_budget.as_mut(),
                )?)
            };
            match setvar.operation {
                SetVarOperation::Delete => {
                    staged.delete(&key, &self.request, self.json_budget.as_mut(), journal.as_deref_mut())?
                }
                SetVarOperation::Assign => {
                    let value = setvar_value(
                        &self.generation,
                        &self.request,
                        staged,
                        setvar,
                        member.runtime_expansion_byte_limit,
                        self.json_budget.as_mut(),
                    )?;
                    staged.set(
                        key.into_owned(),
                        value,
                        self.limits,
                        &self.request,
                        self.json_budget.as_mut(),
                        journal.as_deref_mut(),
                    )?;
                }
                SetVarOperation::Add | SetVarOperation::Subtract => {
                    let current = staged
                        .get(&key, &self.request, self.json_budget.as_mut())?
                        .map(|value| value.bytes(&self.request, self.json_budget.as_mut()))
                        .transpose()?
                        .map_or(0_i64, parse_i64);
                    let delta = if let Some(constant) = setvar.constant {
                        constant
                    } else {
                        let value = setvar_value(
                            &self.generation,
                            &self.request,
                            staged,
                            setvar,
                            member.runtime_expansion_byte_limit,
                            self.json_budget.as_mut(),
                        )?;
                        parse_i64(&value)
                    };
                    // ModSecurity-compatible arithmetic: non-numeric values
                    // count as zero and the result saturates instead of failing.
                    let result = if setvar.operation == SetVarOperation::Add {
                        current.saturating_add(delta)
                    } else {
                        current.saturating_sub(delta)
                    };
                    let (digits, start) = signed_decimal_bytes(result);
                    staged.set(
                        key.into_owned(),
                        owned_copy(&digits[start..])?,
                        self.limits,
                        &self.request,
                        self.json_budget.as_mut(),
                        journal.as_deref_mut(),
                    )?;
                }
            }
        }
        Ok(())
    }

    fn apply_ctl(
        &mut self,
        ctl: CompiledCtl,
        staged: &mut TxState,
        mut journal: Option<&mut TxJournal>,
    ) -> Result<(), RuntimeError> {
        charge_ctl_work(&mut self.ctl_work_bytes, 1, self.limits.max_operator_work_bytes)?;
        match ctl {
            CompiledCtl::ForceRequestBodyVariable(value) => {
                staged.force_request_body_variable = Some(value);
            }
            CompiledCtl::RequestBodyProcessor(value) => {
                if self.next_phase() != Some(Phase::RequestHeaders) || self.request.body.is_some() {
                    return Err(RuntimeError::GenerationInvariant);
                }
                staged.request_body_processor = Some(match value {
                    RequestBodyProcessor::UrlEncoded => BodyProcessorSelection::UrlEncoded,
                    RequestBodyProcessor::Multipart => BodyProcessorSelection::Multipart,
                    RequestBodyProcessor::Json => BodyProcessorSelection::Json,
                    RequestBodyProcessor::Xml => BodyProcessorSelection::Xml,
                });
            }
            CompiledCtl::RuleRemoveByTag(tag) => {
                for entry in tagged_units(&self.generation, tag)? {
                    charge_ctl_work(
                        &mut self.ctl_work_bytes,
                        staged.removed_units.len().saturating_add(1),
                        self.limits.max_operator_work_bytes,
                    )?;
                    staged.add_removed_unit(entry.unit, self.limits, journal.as_deref_mut())?;
                }
            }
            CompiledCtl::RuleRemoveTargetByTag {
                tag,
                collection,
                selector,
            } => {
                let CompiledCtlSelector::Resolved(selector) = selector else {
                    return Err(RuntimeError::GenerationInvariant);
                };
                for entry in tagged_units(&self.generation, tag)? {
                    charge_ctl_work(
                        &mut self.ctl_work_bytes,
                        staged.target_exclusions.len().saturating_add(1),
                        self.limits.max_operator_work_bytes,
                    )?;
                    staged.add_target_exclusion(
                        ActiveTargetExclusion {
                            unit: entry.unit,
                            collection,
                            selector,
                        },
                        self.limits,
                        journal.as_deref_mut(),
                    )?;
                }
            }
        }
        Ok(())
    }
}
