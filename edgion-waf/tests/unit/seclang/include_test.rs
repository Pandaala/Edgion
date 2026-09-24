use std::fs;
use std::path::PathBuf;

use super::*;
use crate::seclang::{Phase, PhaseDefaults};

type FixtureSource = (&'static str, &'static [u8]);
type FixtureEntry = (&'static str, Vec<FixtureSource>);

struct FixtureResolver {
    entries: Vec<FixtureEntry>,
    calls: usize,
    fail: Option<IncludeFailure>,
    ignore_sink_errors: bool,
}

impl IncludeSource for FixtureResolver {
    fn load(&mut self, request: IncludeRequest<'_>, sink: &mut IncludeSink<'_>) -> Result<(), IncludeFailure> {
        self.calls += 1;
        assert!(!format!("{request:?}").contains(request.reference()));
        if let Some(failure) = self.fail {
            return Err(failure);
        }
        let Some((_, sources)) = self
            .entries
            .iter()
            .find(|(reference, _)| *reference == request.reference())
        else {
            return Err(IncludeFailure::NotFound);
        };
        for (identity, bytes) in sources {
            let result = sink.push_source(identity, bytes);
            if !self.ignore_sink_errors {
                result.map_err(|_| IncludeFailure::Other)?;
            }
        }
        Ok(())
    }
}

fn resolver(entries: Vec<FixtureEntry>) -> FixtureResolver {
    FixtureResolver {
        entries,
        calls: 0,
        fail: None,
        ignore_sink_errors: false,
    }
}

fn context() -> SemanticValidationContext {
    let defaults = [
        Phase::RequestHeaders,
        Phase::RequestBody,
        Phase::ResponseHeaders,
        Phase::ResponseBody,
        Phase::Logging,
    ]
    .map(|phase| PhaseDefaults::pass(phase, true, true));
    SemanticValidationContext::try_new(Some(Phase::RequestBody), &defaults).unwrap()
}

#[test]
fn include_expansion_preserves_depth_first_multi_source_rule_order() {
    let root = concat!(
        "SecRule ARGS \"@eq 1\" \"id:1,phase:2,pass\"\n",
        "Include 'bundle' # expansion\n",
        "SecRule ARGS \"@eq 5\" \"id:5,phase:2,pass\"\n",
    );
    let mut includes = resolver(vec![
        (
            "bundle",
            vec![
                (
                    "a.conf",
                    b"SecRule ARGS \"@eq 2\" \"id:2,phase:2,pass\"\nInclude nested\n",
                ),
                ("b.conf", b"SecRule ARGS \"@eq 4\" \"id:4,phase:2,pass\"\n"),
            ],
        ),
        (
            "nested",
            vec![("nested.conf", b"SecRule ARGS \"@eq 3\" \"id:3,phase:2,pass\"\n")],
        ),
    ]);
    let bundle = compile_with_includes(
        "root.conf",
        root.as_bytes(),
        &mut includes,
        None,
        context(),
        FrontendLimits::default(),
    )
    .unwrap();
    assert_eq!(
        bundle
            .generation()
            .rule_units()
            .map(|unit| unit.id())
            .collect::<Vec<_>>(),
        [1, 2, 3, 4, 5]
    );
    assert_eq!(includes.calls, 2);
    assert_eq!(
        bundle.compatibility().include_metrics(),
        IncludeMetrics {
            source_count: 3,
            source_bytes: 138,
            expanded_bytes: 205,
            maximum_depth: 2,
        }
    );
    assert!(bundle.compatibility().is_supported());
}

#[test]
fn optional_missing_include_is_empty_but_required_missing_is_an_error() {
    let mut optional = resolver(Vec::new());
    let report = inspect_compatibility(
        "root",
        b"IncludeOptional missing\nSecAction \"id:1,phase:2,pass\"\n",
        &mut optional,
        FrontendLimits::default(),
    )
    .unwrap();
    assert_eq!(report.rule_count(), 1);
    let mut required = resolver(Vec::new());
    assert!(matches!(
        inspect_compatibility("root", b"Include missing\n", &mut required, FrontendLimits::default(),),
        Err(FrontendError::Include(IncludeError::Source {
            failure: IncludeFailure::NotFound,
            ..
        }))
    ));
}

#[test]
fn include_cycles_depth_count_and_identity_work_are_bounded() {
    let mut cycle = resolver(vec![
        ("a", vec![("a.conf", b"Include b\n")]),
        ("b", vec![("b.conf", b"Include a\n")]),
    ]);
    assert!(matches!(
        inspect_compatibility("root", b"Include a\n", &mut cycle, FrontendLimits::default(),),
        Err(FrontendError::Include(IncludeError::Cycle { depth: 3 }))
    ));

    let mut depth = resolver(vec![
        ("a", vec![("a.conf", b"Include b\n")]),
        ("b", vec![("b.conf", b"SecAction \"id:1,phase:2,pass\"\n")]),
    ]);
    assert!(matches!(
        inspect_compatibility(
            "root",
            b"Include a\n",
            &mut depth,
            FrontendLimits {
                includes: IncludeLimits {
                    max_depth: 1,
                    ..IncludeLimits::default()
                },
                ..FrontendLimits::default()
            },
        ),
        Err(FrontendError::Include(IncludeError::DepthLimit { actual: 2, limit: 1 }))
    ));

    let mut count = resolver(vec![("many", vec![("a.conf", b""), ("b.conf", b"")])]);
    assert!(matches!(
        inspect_compatibility(
            "root",
            b"Include many\n",
            &mut count,
            FrontendLimits {
                includes: IncludeLimits {
                    max_sources: 1,
                    ..IncludeLimits::default()
                },
                ..FrontendLimits::default()
            },
        ),
        Err(FrontendError::Include(IncludeError::Sink {
            error: IncludeSinkError::SourceLimit { limit: 1 },
            ..
        }))
    ));

    let mut identity = resolver(vec![("a", vec![("child", b"")])]);
    assert!(matches!(
        inspect_compatibility(
            "root",
            b"Include a\n",
            &mut identity,
            FrontendLimits {
                includes: IncludeLimits {
                    max_identity_comparison_bytes: 0,
                    ..IncludeLimits::default()
                },
                ..FrontendLimits::default()
            },
        ),
        Err(FrontendError::Include(IncludeError::IdentityComparisonLimit {
            limit: 0
        }))
    ));
}

#[test]
fn include_source_and_expanded_byte_limits_are_exact() {
    let child = b"SecAction \"id:1,phase:2,pass\"\n";
    let mut exact = resolver(vec![("child", vec![("child.conf", child)])]);
    inspect_compatibility(
        "root",
        b"Include child\n",
        &mut exact,
        FrontendLimits {
            includes: IncludeLimits {
                max_source_bytes: child.len(),
                max_expanded_bytes: child.len(),
                ..IncludeLimits::default()
            },
            ..FrontendLimits::default()
        },
    )
    .unwrap();
    let mut excess = resolver(vec![("child", vec![("child.conf", child)])]);
    assert!(matches!(
        inspect_compatibility(
            "root",
            b"Include child\n",
            &mut excess,
            FrontendLimits {
                includes: IncludeLimits {
                    max_source_bytes: child.len() - 1,
                    ..IncludeLimits::default()
                },
                ..FrontendLimits::default()
            },
        ),
        Err(FrontendError::Include(IncludeError::Sink {
            error: IncludeSinkError::SourceByteLimit { .. },
            ..
        }))
    ));

    let mut expanded = resolver(vec![("child", vec![("child.conf", child)])]);
    assert!(matches!(
        inspect_compatibility(
            "root",
            b"Include child\n",
            &mut expanded,
            FrontendLimits {
                includes: IncludeLimits {
                    max_source_bytes: child.len(),
                    max_expanded_bytes: child.len() - 1,
                    ..IncludeLimits::default()
                },
                ..FrontendLimits::default()
            },
        ),
        Err(FrontendError::Include(IncludeError::Sink {
            error: IncludeSinkError::AggregateByteLimit { .. },
            ..
        }))
    ));
}

#[test]
fn multi_source_boundaries_insert_one_separator_when_needed() {
    let mut includes = resolver(vec![(
        "bundle",
        vec![
            ("a.conf", b"SecAction \"id:1,phase:2,pass\""),
            ("b.conf", b"SecAction \"id:2,phase:2,pass\""),
        ],
    )]);
    let bundle = compile_with_includes(
        "root",
        b"Include bundle\n",
        &mut includes,
        None,
        context(),
        FrontendLimits::default(),
    )
    .unwrap();
    assert_eq!(
        bundle
            .generation()
            .rule_units()
            .map(|unit| unit.id())
            .collect::<Vec<_>>(),
        [1, 2]
    );
}

#[test]
fn sticky_sink_failure_precedes_provider_failure_and_redacts_names() {
    let mut includes = FixtureResolver {
        entries: vec![(
            "private-reference",
            vec![(
                "private-identity",
                b"this included source is deliberately larger than the root source",
            )],
        )],
        calls: 0,
        fail: None,
        ignore_sink_errors: true,
    };
    let error = inspect_compatibility(
        "private-root",
        b"Include private-reference\n",
        &mut includes,
        FrontendLimits {
            includes: IncludeLimits {
                max_source_bytes: 26,
                ..IncludeLimits::default()
            },
            ..FrontendLimits::default()
        },
    )
    .unwrap_err();
    let rendered = format!("{error:?} {error}");
    assert!(
        matches!(
            &error,
            FrontendError::Include(IncludeError::Sink {
                error: IncludeSinkError::SourceByteLimit { .. },
                ..
            })
        ),
        "{error:?}"
    );
    assert!(!rendered.contains("private"));
}

#[test]
fn compatibility_report_is_bounded_and_compilation_remains_fail_closed() {
    let source = concat!(
        "SecRule RESPONSE_BODY \"@rx private-pattern\" \"id:1,phase:4,pass\"\n",
        "SecRule RESPONSE_BODY \"@rx another-pattern\" \"id:2,phase:4,pass\"\n",
    );
    let mut inspect_resolver = resolver(Vec::new());
    let report = inspect_compatibility(
        "private-root",
        source.as_bytes(),
        &mut inspect_resolver,
        FrontendLimits {
            compatibility: CompatibilityLimits {
                max_findings: 1,
                max_rule_summaries: 1,
            },
            ..FrontendLimits::default()
        },
    )
    .unwrap();
    assert!(!report.is_supported());
    assert_eq!(report.unavailable_count(UnavailableCapability::FullResponseBody), 2);
    assert_eq!(report.findings().len(), 1);
    assert_eq!(report.omitted_findings(), 1);
    assert_eq!(report.rule_summaries().len(), 1);
    assert_eq!(report.omitted_rule_summaries(), 1);
    let rendered = format!("{report:?}");
    assert!(!rendered.contains("private-pattern"));
    assert!(!rendered.contains("private-root"));

    let mut compile_resolver = resolver(Vec::new());
    assert!(matches!(
        compile_with_includes(
            "private-root",
            source.as_bytes(),
            &mut compile_resolver,
            None,
            context(),
            FrontendLimits::default(),
        ),
        Err(FrontendError::Validation(
            SemanticValidationError::UnavailableCapability { .. }
        ))
    ));

    let mut profile_resolver = resolver(Vec::new());
    assert!(matches!(
        compile_executable_profile_with_includes(
            "private-root",
            source.as_bytes(),
            &mut profile_resolver,
            None,
            context(),
            FrontendLimits {
                compatibility: CompatibilityLimits {
                    max_rule_summaries: 1,
                    ..CompatibilityLimits::default()
                },
                ..FrontendLimits::default()
            },
        ),
        Err(FrontendError::Compatibility(CompatibilityError::RuleSummaryLimit {
            limit: 1
        }))
    ));
}

#[test]
fn requirement_discharge_is_location_keyed_across_compilation_stages() {
    let source = concat!(
        "SecRule REQUEST_URI \"@rx grouped\" \"id:1,phase:1,pass\"\n",
        "SecRule REQUEST_URI \"@rx (captured)\" \"id:2,phase:1,pass,capture\"\n",
    );
    let mut includes = resolver(Vec::new());
    let bundle = compile_with_includes(
        "requirements",
        source.as_bytes(),
        &mut includes,
        None,
        context(),
        FrontendLimits::default(),
    )
    .unwrap();

    assert_eq!(
        bundle
            .generation()
            .rule_units()
            .map(|unit| unit.id())
            .collect::<Vec<_>>(),
        [1, 2]
    );
}

#[test]
fn executable_profile_is_explicit_attributed_and_cannot_weaken_strict_compilation() {
    let source = concat!(
        "SecRule REQUEST_URI \"@contains allowed\" \"id:1,phase:1,pass\"\n",
        "SecRule RESPONSE_BODY \"@contains private\" \"id:2,phase:4,pass\"\n",
        "SecRule ARGS \"@detectSQLi\" \"id:3,phase:2,pass\"\n",
        "SecAction \"id:4,phase:1,pass,ctl:auditEngine=Off\"\n",
        "SecAction \"id:5,phase:1,pass,initcol:global=global\"\n",
    );
    let mut strict_includes = resolver(Vec::new());
    assert!(matches!(
        compile_with_includes(
            "profile",
            source.as_bytes(),
            &mut strict_includes,
            None,
            context(),
            FrontendLimits::default(),
        ),
        Err(FrontendError::Validation(
            SemanticValidationError::UnavailableCapability { .. }
        ))
    ));

    let mut profile_includes = resolver(Vec::new());
    let bundle = compile_executable_profile_with_includes(
        "profile",
        source.as_bytes(),
        &mut profile_includes,
        None,
        context(),
        FrontendLimits::default(),
    )
    .unwrap();
    assert_eq!(
        bundle
            .generation()
            .rule_units()
            .map(|unit| unit.id())
            .collect::<Vec<_>>(),
        [1, 3]
    );
    assert_eq!(
        (
            bundle.compatibility().rule_unit_count(),
            bundle.compatibility().unavailable_rule_unit_count(),
            bundle.compatibility().locally_executable_rule_unit_count(),
        ),
        (5, 3, 2)
    );
    assert_eq!(
        bundle
            .compatibility()
            .rule_summaries()
            .map(CompatibilityRuleSummary::rule_id)
            .collect::<Vec<_>>(),
        [2, 4, 5]
    );
}

#[test]
fn malformed_include_and_continued_text_do_not_escape_the_parser_contract() {
    let mut malformed = resolver(Vec::new());
    assert!(matches!(
        inspect_compatibility(
            "root",
            b"Include 'unterminated\n",
            &mut malformed,
            FrontendLimits::default(),
        ),
        Err(FrontendError::Include(IncludeError::Syntax { line: 1 }))
    ));

    let mut continued = resolver(Vec::new());
    let error = inspect_compatibility(
        "root",
        b"SecAction \"id:1,phase:2,pass\" \\\nInclude should-not-load\n",
        &mut continued,
        FrontendLimits::default(),
    )
    .unwrap_err();
    assert_eq!(continued.calls, 0);
    assert!(matches!(error, FrontendError::Syntax(_) | FrontendError::Semantic(_)));
}

#[test]
fn pinned_crs_compatibility_report_matches_the_frozen_inventory() {
    let Some(root) = std::env::var_os("EDGION_WAF_CRS_RULES_DIR") else {
        return;
    };
    struct OwnedResolver {
        entries: Vec<(String, Vec<u8>)>,
    }
    impl IncludeSource for OwnedResolver {
        fn load(&mut self, request: IncludeRequest<'_>, sink: &mut IncludeSink<'_>) -> Result<(), IncludeFailure> {
            let Some((identity, bytes)) = self
                .entries
                .iter()
                .find(|(identity, _)| identity == request.reference())
            else {
                return Err(IncludeFailure::NotFound);
            };
            sink.push_source(identity, bytes).map_err(|_| IncludeFailure::Other)
        }
    }
    struct OwnedAssets {
        root: PathBuf,
    }
    impl PhraseAssetSource for OwnedAssets {
        fn load(
            &mut self,
            request: PhraseAssetRequest<'_>,
            sink: &mut PhraseAssetSink<'_>,
        ) -> Result<(), PhraseAssetFailure> {
            let handle = PathBuf::from(request.handle());
            if handle.components().count() != 1 {
                return Err(PhraseAssetFailure::Denied);
            }
            let bytes = fs::read(self.root.join(handle)).map_err(|_| PhraseAssetFailure::NotFound)?;
            sink.extend_from_slice(&bytes).map_err(|_| PhraseAssetFailure::Other)
        }
    }

    let root = PathBuf::from(root);
    let mut paths = fs::read_dir(&root)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "conf"))
        .collect::<Vec<_>>();
    paths.sort();
    let mut include_root = String::new();
    let mut entries = Vec::new();
    for path in paths {
        let identity = path.strip_prefix(&root).unwrap().to_string_lossy().into_owned();
        include_root.push_str("Include '");
        include_root.push_str(&identity);
        include_root.push_str("'\n");
        entries.push((identity, fs::read(path).unwrap()));
    }
    let mut includes = OwnedResolver {
        entries: entries.clone(),
    };
    let report = inspect_compatibility(
        "crs-rules",
        include_root.as_bytes(),
        &mut includes,
        FrontendLimits::default(),
    )
    .unwrap();
    assert_eq!(
        [
            report.unavailable_count(UnavailableCapability::FullResponseBody),
            report.unavailable_count(UnavailableCapability::Ctl),
            report.unavailable_count(UnavailableCapability::InitCol),
        ],
        [59, 18, 2]
    );
    assert_eq!(report.findings().len(), 79);
    assert_eq!(report.omitted_findings(), 0);
    assert_eq!(
        (
            report.rule_unit_count(),
            report.unavailable_rule_unit_count(),
            report.locally_executable_rule_unit_count(),
            report.unattributed_findings(),
        ),
        (627, 68, 559, 0)
    );
    let unavailable_rule_counts = [
        UnavailableCapability::FullResponseBody,
        UnavailableCapability::Ctl,
        UnavailableCapability::InitCol,
    ]
    .map(|capability| {
        report
            .findings()
            .filter(|finding| finding.capability() == capability)
            .filter_map(CompatibilityFinding::rule_id)
            .collect::<std::collections::BTreeSet<_>>()
            .len()
    });
    assert_eq!(unavailable_rule_counts, [59, 8, 1]);
    assert_eq!(report.rule_summaries().len(), 68);
    assert_eq!(report.omitted_rule_summaries(), 0);
    for summary in report.rule_summaries() {
        assert!([
            UnavailableCapability::FullResponseBody,
            UnavailableCapability::Ctl,
            UnavailableCapability::InitCol,
        ]
        .into_iter()
        .any(|capability| summary.has_capability(capability)));
    }
    assert!(!report.is_supported());

    let mut includes = OwnedResolver { entries };
    let mut assets = OwnedAssets { root: root.clone() };
    let profile = compile_executable_profile_with_includes(
        "crs-rules",
        include_root.as_bytes(),
        &mut includes,
        Some(&mut assets),
        context(),
        FrontendLimits::default(),
    )
    .unwrap();
    assert_eq!(profile.generation().rule_unit_count(), 559);
    assert_eq!(profile.compatibility(), &report);
}
