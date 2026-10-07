use super::*;
use crate::seclang::{
    compile_with_includes, FrontendLimits, IncludeFailure, IncludeRequest, IncludeSink, IncludeSource, PhaseDefaults,
    SemanticValidationContext,
};
struct NoIncludes;
impl IncludeSource for NoIncludes {
    fn load(&mut self, _: IncludeRequest<'_>, _: &mut IncludeSink<'_>) -> Result<(), IncludeFailure> {
        Err(IncludeFailure::NotFound)
    }
}
fn compile(rules: &str) -> CompiledGeneration {
    let phases = [
        Phase::RequestHeaders,
        Phase::RequestBody,
        Phase::ResponseHeaders,
        Phase::ResponseBody,
        Phase::Logging,
    ]
    .map(|p| PhaseDefaults::pass(p, true, true));
    compile_with_includes(
        "activation",
        rules.as_bytes(),
        &mut NoIncludes,
        None,
        SemanticValidationContext::try_new(Some(Phase::RequestBody), &phases).unwrap(),
        FrontendLimits::default(),
    )
    .unwrap()
    .into_parts()
    .0
}
fn gate_with(body: &str) -> CompiledGeneration {
    compile(&format!(
        "SecRule TX:level \"!@ge 2\" \"id:1,phase:2,pass,t:none,skipAfter:END\"\n{body}\nSecMarker END\n"
    ))
}
#[test]
fn generic_gate_eligibility_and_negation_use_canonical_integer_relation() {
    let g = gate_with("SecRule ARGS \"@contains attack\" \"id:2,phase:2,pass\"");
    let gate = g.inspection_activation.steps[0].gate.unwrap();
    assert!(gate.negated);
    assert_eq!(gate.expected, 2);
    assert_eq!(
        g.inspection_activation.steps[0].matched,
        g.inspection_activation.steps.len()
    );
    assert!(g.inspection_activation.has_gate);
    for rule in [
        "SecRule TX:level \"@ge %{TX.other}\" \"id:1,phase:2,pass,skipAfter:END\"",
        "SecRule &TX:level \"@ge 2\" \"id:1,phase:2,pass,skipAfter:END\"",
        "SecRule TX:level \"@ge 2\" \"id:1,phase:2,pass,t:length,skipAfter:END\"",
        "SecRule TX:level|TX:other \"@ge 2\" \"id:1,phase:2,pass,skipAfter:END\"",
        "SecRule TX:level \"@ge 2\" \"id:1,phase:2,pass,skipAfter:END,chain\"\nSecRule ARGS \"@contains attack\" \"t:none\"",
    ] {
        let g = compile(&format!("{rule}\nSecMarker END\n"));
        assert!(g.inspection_activation.steps[0].gate.is_none(), "{rule}");
    }
}
#[test]
fn all_phase_two_writes_and_case_aliases_invalidate_stability() {
    for action in [
        "setvar:tx.LEVEL=3",
        "setvar:!tx.LeVeL",
        "setvar:tx.level=+1",
        "setvar:tx.level=-1",
        "setvar:tx.%{TX.name}=3",
        "setvar:tx.le%{TX.name}vel=3",
    ] {
        let g = gate_with(&format!("SecRule ARGS \"@contains attack\" \"id:2,phase:2,pass,chain\"\nSecRule REQUEST_METHOD \"@streq POST\" \"{action}\""));
        assert!(g.inspection_activation.steps[0].gate.is_none(), "{action}");
    }
    for key in [
        "score_%{TX.name}",
        "%{TX.name}_score",
        "lev%{TX.name}extra",
        "longer_than_level%{TX.name}",
        "x%{TX.name}level",
    ] {
        let g = gate_with(&format!("SecAction \"id:2,phase:2,pass,setvar:tx.{key}=3\""));
        assert!(g.inspection_activation.steps[0].gate.is_some(), "{key}");
    }
    let g = gate_with("SecAction \"id:2,phase:3,pass,setvar:tx.level=3\"");
    assert!(g.inspection_activation.steps[0].gate.is_some());
}
#[test]
fn generation_capture_aliases_cover_prior_cleanup_numeric_named_and_detector() {
    for (key, capture) in [
        (
            "level",
            "SecRule REQUEST_URI \"@rx (?P<LeVeL>.)\" \"id:9,phase:1,pass,capture\"",
        ),
        ("1", "SecRule ARGS \"@rx (.)\" \"id:9,phase:2,pass,capture\""),
        ("0", "SecRule ARGS \"@pm hit\" \"id:9,phase:2,pass,capture\""),
        ("0", "SecRule ARGS \"@detectSQLi\" \"id:9,phase:2,pass,capture\""),
    ] {
        let g = compile(&format!(
            "{capture}\nSecRule TX:{key} \"@ge 2\" \"id:1,phase:2,pass,skipAfter:END\"\nSecMarker END\n"
        ));
        assert!(!g.inspection_activation.has_gate, "{key} {capture}");
    }
    let g = compile("SecRule TX:01 \"@ge 2\" \"id:1,phase:2,pass,skipAfter:END\"\nSecRule ARGS \"@rx (.)\" \"id:2,phase:2,pass,capture\"\nSecMarker END\n");
    assert!(g.inspection_activation.has_gate);
}
#[test]
fn phase_two_removal_controls_disable_gates_without_disabling_schedule_proof() {
    let g = gate_with("SecAction \"id:2,phase:2,pass,ctl:ruleRemoveByTag=gate\"");
    assert!(!g.inspection_activation.has_gate);
    assert_eq!(
        g.inspection_activation.steps[0].matched,
        g.inspection_activation.steps.len()
    );
}
#[test]
fn successor_shares_marker_skip_count_and_chain_precedence() {
    let mut g = compile(concat!(
        "SecRule ARGS \"@contains attack\" \"id:1,phase:2,pass,skip:1,chain\"\n",
        "SecRule ARGS \"@contains attack\" \"setvar:tx.observed=yes\"\n",
        "SecMarker A\n",
        "SecAction \"id:2,phase:2,pass\"\n",
        "SecMarker B\n",
        "SecAction \"id:3,phase:2,pass\"\n",
        "SecAction \"id:4,phase:2,pass\"\n",
    ));
    // Child skip is currently rejected by grammar; exercise the shared pure
    // successor directly on arena records to freeze precedence regardless.
    let action = g.members[1].action_start;
    g.actions[action].operand = CompiledActionOperand::Local(LocalActionValue::Skip(2));
    assert_eq!(successor(&g, g.units[0], 1, 1, |_| Ok(()), invalid).unwrap(), 5);
    let mut g = compile(concat!(
        "SecRule ARGS \"@contains attack\" \"id:1,phase:2,pass,skipAfter:A,chain\"\n",
        "SecRule ARGS \"@contains attack\" \"setvar:tx.observed=yes\"\n",
        "SecMarker A\n",
        "SecAction \"id:2,phase:2,pass\"\n",
        "SecMarker B\n",
        "SecAction \"id:3,phase:2,pass\"\n",
    ));
    g.members[1].skip_after_marker = Some(3);
    assert_eq!(successor(&g, g.units[0], 1, 1, |_| Ok(()), invalid).unwrap(), 2);
}

#[test]
fn non_tx_target_controls_before_and_after_gate_cannot_disable_tx_gate() {
    for ctl in [
        "ctl:ruleRemoveTargetByTag=gate;XML://@*",
        "ctl:ruleRemoveTargetByTag=gate;ARGS:ignored",
    ] {
        for before in [false, true] {
            let control = format!("SecAction \"id:2,phase:2,pass,{ctl}\"\n");
            let gate = "SecRule TX:level \"@lt 2\" \"id:1,phase:2,pass,tag:gate,skipAfter:END\"\n";
            let rules = if before {
                format!("{control}{gate}SecMarker END\n")
            } else {
                format!("{gate}SecMarker END\n{control}")
            };
            let g = compile(&rules);
            assert!(g.inspection_activation.has_gate, "{before} {ctl}");
        }
    }
    for ctl in ["ctl:ruleRemoveByTag=gate", "ctl:ruleRemoveTargetByTag=gate;TX:level"] {
        let g = gate_with(&format!("SecAction \"id:2,phase:2,pass,{ctl}\""));
        assert!(!g.inspection_activation.has_gate, "{ctl}");
    }
}

#[test]
fn activation_compile_proof_work_memory_and_peak_exact_boundaries() {
    let g = gate_with("SecAction \"id:2,phase:2,pass,setvar:tx.score_%{TX.name}=3\"\nSecRule ARGS \"@rx (?P<word>attack)\" \"id:3,phase:2,pass,capture\"");
    let run = |limits| {
        let mut memory = MatcherCompileWork {
            limits,
            validated_bytes: 0,
            working_bytes: 0,
            compiler_owned_bytes: 0,
            matcher_charged_bytes: 0,
            peak_bytes: 0,
            requirement_cursors: [0; 8],
        };
        let mut work = CanonicalWork::default();
        let result = CompiledActivation::compile(&g, &mut memory, &mut work);
        (result, work, memory)
    };
    let (result, work, memory) = run(CompilerLimits::default());
    assert!(result.unwrap().has_gate);
    assert!(work.record_comparisons > 0);
    assert!(work.comparison_bytes > 0);
    assert!(memory.compiler_owned_bytes > 0);
    let exact = CompilerLimits {
        max_group_key_record_comparisons: work.record_comparisons,
        max_group_key_comparison_bytes: work.comparison_bytes,
        max_compiler_owned_non_matcher_bytes: memory.compiler_owned_bytes,
        max_compilation_peak_bytes: memory.peak_bytes,
        ..CompilerLimits::default()
    };
    assert!(run(exact).0.is_ok());
    assert!(matches!(
        run(CompilerLimits {
            max_group_key_record_comparisons: work.record_comparisons - 1,
            ..exact
        })
        .0,
        Err(CompilerError::GroupKeyRecordComparisonLimit { .. })
    ));
    assert!(matches!(
        run(CompilerLimits {
            max_group_key_comparison_bytes: work.comparison_bytes - 1,
            ..exact
        })
        .0,
        Err(CompilerError::GroupKeyComparisonByteLimit { .. })
    ));
    assert!(matches!(
        run(CompilerLimits {
            max_compiler_owned_non_matcher_bytes: memory.compiler_owned_bytes - 1,
            ..exact
        })
        .0,
        Err(CompilerError::OwnedByteLimit { .. })
    ));
    assert!(matches!(
        run(CompilerLimits {
            max_compilation_peak_bytes: memory.peak_bytes - 1,
            ..exact
        })
        .0,
        Err(CompilerError::PeakByteLimit { .. })
    ));
}
