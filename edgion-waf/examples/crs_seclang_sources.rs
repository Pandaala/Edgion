use std::collections::BTreeMap;
use std::env;
use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};

use edgion_waf::seclang::{
    parse_directives, scan_source, tokenize_document, Directive, DirectiveLimits, SourceLimits, TokenizerLimits,
};

fn main() -> Result<(), Box<dyn Error>> {
    let root = env::args()
        .nth(1)
        .ok_or("usage: crs_seclang_sources <rules-directory>")?;
    let mut files = Vec::new();
    collect_conf_files(Path::new(&root), &mut files)?;
    files.sort();

    let mut physical_lines = 0_usize;
    let mut logical_lines = 0_usize;
    let mut provenance_segments = 0_usize;
    let mut source_charged_bytes = 0_usize;
    let mut tokenizer_charged_bytes = 0_usize;
    let mut directive_charged_bytes = 0_usize;
    let mut max_logical_bytes = 0_usize;
    let mut directives = BTreeMap::<String, usize>::new();
    let mut arities = BTreeMap::<String, usize>::new();
    let mut operators = BTreeMap::<String, usize>::new();
    let mut token_count = 0_usize;
    let mut typed_directives = BTreeMap::<&'static str, usize>::new();
    let mut envelope_rule_target_expressions = 0_usize;
    let mut envelope_update_target_expressions = 0_usize;
    let mut envelope_update_id_selectors = 0_usize;
    for file in &files {
        let source = fs::read(file)?;
        let source_id = file.to_string_lossy();
        let document = scan_source(&source_id, &source, SourceLimits::default())?;
        let tokenized = tokenize_document(&document, TokenizerLimits::default())?;
        let parsed = parse_directives(&tokenized, DirectiveLimits::default())?;
        physical_lines = physical_lines.saturating_add(document.physical_line_count());
        logical_lines = logical_lines.saturating_add(document.lines().len());
        provenance_segments = provenance_segments.saturating_add(document.provenance_segment_count());
        source_charged_bytes = source_charged_bytes.saturating_add(document.charged_bytes());
        tokenizer_charged_bytes = tokenizer_charged_bytes.saturating_add(tokenized.charged_bytes());
        directive_charged_bytes = directive_charged_bytes.saturating_add(parsed.charged_bytes());
        max_logical_bytes =
            max_logical_bytes.max(document.lines().iter().map(|line| line.text().len()).max().unwrap_or(0));
        token_count = token_count.saturating_add(tokenized.tokens().len());
        for view in parsed.iter() {
            let name = match view.directive() {
                Directive::SecRule { .. } => {
                    envelope_rule_target_expressions = envelope_rule_target_expressions.saturating_add(1);
                    "SecRule"
                }
                Directive::SecAction { .. } => "SecAction",
                Directive::SecMarker { .. } => "SecMarker",
                Directive::SecComponentSignature { .. } => "SecComponentSignature",
                Directive::SecRuleUpdateTargetById { selectors, .. } => {
                    envelope_update_target_expressions = envelope_update_target_expressions.saturating_add(1);
                    envelope_update_id_selectors = envelope_update_id_selectors.saturating_add(selectors.len());
                    "SecRuleUpdateTargetById"
                }
                Directive::SecRuleRemoveById { selectors } => {
                    envelope_update_id_selectors = envelope_update_id_selectors.saturating_add(selectors.len());
                    "SecRuleRemoveById"
                }
            };
            *typed_directives.entry(name).or_default() += 1;
        }
        for line_index in 0..tokenized.line_count() {
            let line = tokenized.line(line_index).ok_or("missing tokenized line")?;
            let tokens = line.tokens();
            let directive = tokens.first().map(|token| token.value()).unwrap_or("");
            *directives.entry(directive.to_owned()).or_default() += 1;
            *arities.entry(format!("{directive}/{}", tokens.len())).or_default() += 1;
            if directive.eq_ignore_ascii_case("SecRule") {
                let operator = tokens.get(2).map(|token| token.value()).unwrap_or("");
                let operator_name = operator
                    .strip_prefix('!')
                    .unwrap_or(operator)
                    .split_ascii_whitespace()
                    .next()
                    .unwrap_or("");
                *operators.entry(operator_name.to_owned()).or_default() += 1;
            }
        }
    }

    println!("files={}", files.len());
    println!("physical_lines={physical_lines}");
    println!("logical_lines={logical_lines}");
    println!("provenance_segments={provenance_segments}");
    println!("source_charged_bytes={source_charged_bytes}");
    println!("tokenizer_charged_bytes={tokenizer_charged_bytes}");
    println!("directive_charged_bytes={directive_charged_bytes}");
    println!("max_logical_bytes={max_logical_bytes}");
    println!("tokens={token_count}");
    println!("envelope_rule_target_expressions={envelope_rule_target_expressions}");
    println!("envelope_update_target_expressions={envelope_update_target_expressions}");
    println!("envelope_update_id_selectors={envelope_update_id_selectors}");
    for (directive, count) in typed_directives {
        println!("typed_directive[{directive}]={count}");
    }
    for (directive, count) in directives {
        println!("directive[{directive}]={count}");
    }
    for (shape, count) in arities {
        println!("arity[{shape}]={count}");
    }
    for (operator, count) in operators {
        println!("operator[{operator}]={count}");
    }
    Ok(())
}

fn collect_conf_files(directory: &Path, files: &mut Vec<PathBuf>) -> Result<(), Box<dyn Error>> {
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            collect_conf_files(&entry.path(), files)?;
        } else if file_type.is_file() && entry.path().extension().is_some_and(|extension| extension == "conf") {
            files.try_reserve(1)?;
            files.push(entry.path());
        }
    }
    Ok(())
}
