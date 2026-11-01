//! Local Proxy-Wasm inspection and strict Envoy v3 conversion.
//!
//! This module intentionally has no HTTP client dependency. Both subcommands
//! are dispatched before token resolution and `EdgionClient` construction.

use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use base64::Engine as _;
use clap::{Args, Subcommand, ValueEnum};
use edgion_resources::resources::edgion_plugins::plugin_configs::{
    WasmConfig, WasmFetchRetry, WasmFetchSpec, WasmRawConfig, WasmSourceSpec, MAX_INLINE_DECODED_BYTES,
};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use url::Url;

const MAX_WASM_MODULE_BYTES: u64 = 64 * 1024 * 1024;
const MAX_ENVOY_INPUT_BYTES: u64 = 8 * 1024 * 1024;
const CONVERSION_REPORT_VERSION: u32 = 1;
const ENVOY_PLUGIN_API: &str = "envoy.extensions.wasm.v3.PluginConfig";
const ENVOY_HTTP_WASM_FILTER: &str = "envoy.filters.http.wasm";
const ENVOY_HTTP_WASM_TYPE: &str = "type.googleapis.com/envoy.extensions.filters.http.wasm.v3.Wasm";
const ENVOY_WASMTIME_RUNTIME: &str = "envoy.wasm.runtime.wasmtime";
const ANY_MODE: &str = "wrapper_value_exact_bytes_v1";

#[derive(Debug, Args)]
pub struct WasmArgs {
    #[command(subcommand)]
    pub command: WasmCommand,
}

#[derive(Debug, Subcommand)]
pub enum WasmCommand {
    /// Analyze a Wasm module against Edgion's exact Proxy-Wasm host profile.
    Inspect {
        /// Wasm module to inspect (stdin is not supported).
        #[arg(long, value_name = "MODULE.WASM")]
        file: PathBuf,
        /// Report output format.
        #[arg(long, value_enum, default_value = "text")]
        format: InspectFormat,
    },
    /// Convert one Envoy v3 HTTP Wasm filter or PluginConfig to Edgion WasmConfig.
    ConvertEnvoy {
        /// Envoy YAML or protobuf-JSON input (stdin is not supported).
        #[arg(long, value_name = "ENVOY.YAML")]
        file: PathBuf,
        /// Generated WasmConfig format written to stdout.
        #[arg(long, value_enum, default_value = "yaml")]
        format: ConfigFormat,
        /// Write a stable JSON conversion report without overwriting an existing file.
        #[arg(long, value_name = "REPORT.JSON")]
        report: Option<PathBuf>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum InspectFormat {
    Text,
    Json,
    Yaml,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum ConfigFormat {
    Yaml,
    Json,
}

pub fn execute(args: &WasmArgs) -> Result<()> {
    match &args.command {
        WasmCommand::Inspect { file, format } => inspect(file, *format),
        WasmCommand::ConvertEnvoy { file, format, report } => convert_envoy_file(file, *format, report.as_deref()),
    }
}

fn inspect(path: &Path, format: InspectFormat) -> Result<()> {
    let bytes = read_bounded(path, MAX_WASM_MODULE_BYTES, "Wasm module")?;
    let report = edgion_wasm::analyze(&bytes);
    match format {
        InspectFormat::Text => print!("{}", inspect_text(&report)),
        InspectFormat::Json => println!("{}", serde_json::to_string_pretty(&report)?),
        InspectFormat::Yaml => print!("{}", yaml_serde::to_string(&report)?),
    }
    if !report.compatible {
        bail!("Wasm module is incompatible; see the report above");
    }
    Ok(())
}

fn inspect_text(report: &edgion_wasm::CompatibilityReport) -> String {
    let mut output = format!(
        "reportVersion: {}\ncompatible: {}\nsha256: {}\nabiMarkers: {}\nhooks: {}\nimports: {}\nexports: {}\nwarnings: {}\nerrors: {}\n",
        report.report_version,
        report.compatible,
        report.sha256,
        report.abi_markers.len(),
        report.hooks.len(),
        report.imports.len(),
        report.exports.len(),
        report.warnings.len(),
        report.errors.len(),
    );
    output.push_str(&format!(
        "profile: {}\nassessment: {}\nexecutionVerified: false\n",
        report.profile, report.assessment
    ));
    // Module-controlled strings must never become terminal commands or extra lines.
    // Keep escaping at this presentation boundary; structured reports retain raw values.
    for import in &report.imports {
        output.push_str(&format!(
            "import: {}::{} {:?}\n",
            import.module.escape_debug(),
            import.name.escape_debug(),
            import.support
        ));
    }
    for hook in &report.hooks {
        output.push_str(&format!("hook: {} {:?}\n", hook.name.escape_debug(), hook.support));
    }
    for issue in &report.warnings {
        output.push_str(&format!("warning: {}: {}\n", issue.code, issue.message.escape_debug()));
    }
    for issue in &report.errors {
        output.push_str(&format!("error: {}: {}\n", issue.code, issue.message.escape_debug()));
    }
    output
}

fn convert_envoy_file(path: &Path, format: ConfigFormat, report_path: Option<&Path>) -> Result<()> {
    let input = read_bounded(path, MAX_ENVOY_INPUT_BYTES, "Envoy conversion input")?;
    let outcome = convert_envoy(&input);

    // Finish every serialization before creating any file. The report is then
    // fully written to a same-directory temporary and atomically published
    // with no-clobber semantics, so the target is never partial.
    let report_json = serde_json::to_vec_pretty(&outcome.report).context("failed to serialize conversion report")?;
    let output = match (&outcome.config, format) {
        (Some(config), ConfigFormat::Yaml) => {
            Some(yaml_serde::to_string(config).context("failed to serialize generated WasmConfig")?)
        }
        (Some(config), ConfigFormat::Json) => {
            Some(serde_json::to_string_pretty(config).context("failed to serialize generated WasmConfig")?)
        }
        (None, _) => None,
    };

    if let Some(path) = report_path {
        write_create_new(path, &report_json)?;
    } else {
        // Stdout is reserved for the generated WasmConfig fragment. Without
        // --report, emit the same structured report on stderr so warnings and
        // blockers are never silently discarded.
        eprintln!("{}", String::from_utf8_lossy(&report_json));
    }
    if let Some(output) = output {
        print!("{output}");
        if !output.ends_with('\n') {
            println!();
        }
    }
    if !outcome.report.compatible {
        bail!(
            "Envoy Wasm conversion blocked by {} finding(s); no configuration was generated",
            outcome.report.counts.blockers
        );
    }
    Ok(())
}

fn write_create_new(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let mut temporary = tempfile::NamedTempFile::new_in(parent).with_context(|| {
        format!(
            "cannot create a temporary conversion report beside '{}'",
            path.display()
        )
    })?;
    temporary
        .write_all(bytes)
        .and_then(|_| temporary.write_all(b"\n"))
        .and_then(|_| temporary.as_file().sync_all())
        .context("failed to write the temporary conversion report")?;
    temporary.persist_noclobber(path).map_err(|error| {
        anyhow::anyhow!(
            "cannot create conversion report '{}'; the path must not already exist: {}",
            path.display(),
            error.error
        )
    })?;
    Ok(())
}

fn read_bounded(path: &Path, cap: u64, label: &str) -> Result<Vec<u8>> {
    if path.as_os_str() == "-" {
        bail!("{label} stdin input is not supported; pass a file path");
    }
    let metadata =
        fs::metadata(path).with_context(|| format!("cannot inspect {} '{}'", label.to_lowercase(), path.display()))?;
    if metadata.len() > cap {
        bail!("{label} exceeds the {}MiB size limit", cap / 1024 / 1024);
    }
    let file =
        File::open(path).with_context(|| format!("cannot open {} '{}'", label.to_lowercase(), path.display()))?;
    let mut bytes = Vec::with_capacity(metadata.len().min(cap) as usize);
    file.take(cap + 1)
        .read_to_end(&mut bytes)
        .with_context(|| format!("cannot read {} '{}'", label.to_lowercase(), path.display()))?;
    if bytes.len() as u64 > cap {
        bail!("{label} exceeds the {}MiB size limit", cap / 1024 / 1024);
    }
    Ok(bytes)
}

#[derive(Debug)]
struct ConversionOutcome {
    config: Option<WasmConfig>,
    report: EnvoyWasmConversionReport,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct EnvoyWasmConversionReport {
    pub version: u32,
    pub input_sha256: String,
    pub target_envoy_api: String,
    pub any_semantics_mode: String,
    pub compatible: bool,
    pub output_generated: bool,
    pub findings: Vec<ConversionFinding>,
    pub counts: FindingCounts,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ConversionFinding {
    pub code: String,
    pub level: FindingLevel,
    pub source_path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target_path: Option<String>,
    pub message: String,
    pub remediation: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FindingLevel {
    Warning,
    Blocker,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub struct FindingCounts {
    pub warnings: usize,
    pub blockers: usize,
}

struct ReportBuilder {
    input_sha256: String,
    findings: Vec<ConversionFinding>,
}

impl ReportBuilder {
    fn new(input: &[u8]) -> Self {
        Self {
            input_sha256: hex::encode(Sha256::digest(input)),
            findings: Vec::new(),
        }
    }

    fn warning(&mut self, code: &str, source_path: &str, target_path: Option<&str>, message: &str, remediation: &str) {
        self.push(
            code,
            FindingLevel::Warning,
            source_path,
            target_path,
            message,
            remediation,
        );
    }

    fn blocker(&mut self, code: &str, source_path: &str, target_path: Option<&str>, message: &str, remediation: &str) {
        self.push(
            code,
            FindingLevel::Blocker,
            source_path,
            target_path,
            message,
            remediation,
        );
    }

    fn push(
        &mut self,
        code: &str,
        level: FindingLevel,
        source_path: &str,
        target_path: Option<&str>,
        message: &str,
        remediation: &str,
    ) {
        self.findings.push(ConversionFinding {
            code: code.to_string(),
            level,
            source_path: source_path.to_string(),
            target_path: target_path.map(str::to_string),
            message: message.to_string(),
            remediation: remediation.to_string(),
        });
    }

    fn finish(mut self, config: Option<WasmConfig>) -> ConversionOutcome {
        if config.is_none()
            && !self
                .findings
                .iter()
                .any(|finding| finding.level == FindingLevel::Blocker)
        {
            self.blocker(
                "output_not_generated",
                "/",
                Some("/"),
                "Conversion completed without generating a WasmConfig.",
                "Report this internal converter error; no output is safe to deploy.",
            );
        }
        let counts = FindingCounts {
            warnings: self
                .findings
                .iter()
                .filter(|finding| finding.level == FindingLevel::Warning)
                .count(),
            blockers: self
                .findings
                .iter()
                .filter(|finding| finding.level == FindingLevel::Blocker)
                .count(),
        };
        let compatible = counts.blockers == 0;
        let config = if compatible { config } else { None };
        ConversionOutcome {
            report: EnvoyWasmConversionReport {
                version: CONVERSION_REPORT_VERSION,
                input_sha256: self.input_sha256,
                target_envoy_api: ENVOY_PLUGIN_API.to_string(),
                any_semantics_mode: ANY_MODE.to_string(),
                compatible,
                output_generated: compatible && config.is_some(),
                findings: self.findings,
                counts,
            },
            config,
        }
    }
}

fn convert_envoy(input: &[u8]) -> ConversionOutcome {
    let mut report = ReportBuilder::new(input);
    let value: Value = match yaml_serde::from_slice(input) {
        Ok(value) => value,
        Err(_) => {
            report.blocker(
                "input_parse_failed",
                "/",
                None,
                "Input is not valid YAML or JSON for the supported Envoy v3 shape.",
                "Provide one HTTP Wasm filter wrapper, typed_config, or bare v3 PluginConfig.",
            );
            return report.finish(None);
        }
    };
    let Some(plugin) = parse_supported_input(value, &mut report) else {
        return report.finish(None);
    };
    let config = convert_plugin(plugin, &mut report);
    if let Some(config) = &config {
        if config.validate().is_err() {
            report.blocker(
                "generated_config_invalid",
                "/",
                Some("/"),
                "The mechanically generated WasmConfig did not pass Edgion validation.",
                "Review the source, timeout, retry, digest, and configuration byte limits.",
            );
        }
    }
    report.finish(config)
}

fn parse_supported_input(value: Value, report: &mut ReportBuilder) -> Option<PluginConfigDto> {
    let Some(object) = value.as_object() else {
        report.blocker(
            "unsupported_input_shape",
            "/",
            None,
            "The converter accepts exactly one object.",
            "Provide one HTTP Wasm filter wrapper, typed_config, or bare v3 PluginConfig.",
        );
        return None;
    };
    if object.contains_key("typed_config") || object.contains_key("typedConfig") {
        let wrapper: HttpFilterWrapperDto = match serde_json::from_value(value) {
            Ok(wrapper) => wrapper,
            Err(_) => {
                schema_error(report, "/");
                return None;
            }
        };
        if wrapper.name != ENVOY_HTTP_WASM_FILTER {
            report.blocker(
                "wrong_http_filter_name",
                "/name",
                None,
                "The wrapper name is not the Envoy HTTP Wasm filter.",
                "Set name to envoy.filters.http.wasm.",
            );
        }
        return unwrap_typed_config(wrapper.typed_config, "/typed_config", report);
    }
    if object.contains_key("@type") || object.contains_key("type_url") || object.contains_key("typeUrl") {
        let typed: HttpWasmTypedConfigDto = match serde_json::from_value(value) {
            Ok(typed) => typed,
            Err(_) => {
                schema_error(report, "/");
                return None;
            }
        };
        return unwrap_typed_config(typed, "/", report);
    }
    match serde_json::from_value(value) {
        Ok(plugin) => Some(plugin),
        Err(_) => {
            schema_error(report, "/");
            None
        }
    }
}

fn schema_error(report: &mut ReportBuilder, path: &str) {
    report.blocker(
        "input_schema_invalid",
        path,
        None,
        "The supported Envoy v3 object contains an unknown field, invalid enum, or invalid value type.",
        "Remove unsupported fields and use official snake_case YAML or protobuf JSON camelCase names.",
    );
}

fn unwrap_typed_config(
    typed: HttpWasmTypedConfigDto,
    path: &str,
    report: &mut ReportBuilder,
) -> Option<PluginConfigDto> {
    if typed.type_url != ENVOY_HTTP_WASM_TYPE {
        report.blocker(
            "wrong_typed_config",
            &format!("{path}/@type"),
            None,
            "typed_config is not envoy.extensions.filters.http.wasm.v3.Wasm.",
            "Provide the v3 HTTP Wasm typed_config.",
        );
    }
    if typed.config.is_none() {
        report.blocker(
            "missing_plugin_config",
            &format!("{path}/config"),
            None,
            "The HTTP Wasm typed_config has no PluginConfig.",
            "Set typed_config.config to one v3 PluginConfig.",
        );
    }
    typed.config
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct HttpFilterWrapperDto {
    name: String,
    #[serde(alias = "typedConfig")]
    typed_config: HttpWasmTypedConfigDto,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct HttpWasmTypedConfigDto {
    #[serde(rename = "@type", alias = "type_url", alias = "typeUrl")]
    type_url: String,
    config: Option<PluginConfigDto>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct PluginConfigDto {
    #[serde(default)]
    name: String,
    #[serde(default, alias = "rootId")]
    root_id: String,
    #[serde(default, alias = "vmConfig")]
    vm_config: Option<VmConfigDto>,
    #[serde(default)]
    configuration: Option<Value>,
    #[serde(default, alias = "failOpen")]
    fail_open: bool,
    #[serde(default, alias = "failurePolicy")]
    failure_policy: Option<String>,
    #[serde(default, alias = "reloadConfig")]
    reload_config: Option<ReloadConfigDto>,
    #[serde(default, alias = "capabilityRestrictionConfig")]
    capability_restriction_config: Option<CapabilityRestrictionDto>,
    #[serde(default, alias = "allowOnHeadersStopIteration")]
    allow_on_headers_stop_iteration: Option<bool>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct VmConfigDto {
    #[serde(default, alias = "vmId")]
    vm_id: String,
    #[serde(default)]
    runtime: String,
    #[serde(default)]
    code: Option<AsyncDataSourceDto>,
    #[serde(default)]
    configuration: Option<Value>,
    #[serde(default, alias = "allowPrecompiled")]
    allow_precompiled: bool,
    #[serde(default, alias = "nackOnCodeCacheMiss")]
    nack_on_code_cache_miss: bool,
    #[serde(default, alias = "environmentVariables")]
    environment_variables: Option<EnvironmentVariablesDto>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct EnvironmentVariablesDto {
    #[serde(default, alias = "hostEnvKeys")]
    host_env_keys: Vec<String>,
    #[serde(default, alias = "keyValues")]
    key_values: Map<String, Value>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReloadConfigDto {
    #[serde(default)]
    backoff: Option<BackoffStrategyDto>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CapabilityRestrictionDto {
    #[serde(default, alias = "allowedCapabilities")]
    allowed_capabilities: std::collections::BTreeMap<String, SanitizationConfigDto>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SanitizationConfigDto {}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct AsyncDataSourceDto {
    #[serde(default)]
    local: Option<LocalDataSourceDto>,
    #[serde(default)]
    remote: Option<RemoteDataSourceDto>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct LocalDataSourceDto {
    #[serde(default)]
    filename: Option<String>,
    #[serde(default, alias = "inlineBytes")]
    inline_bytes: Option<String>,
    #[serde(default, alias = "inlineString")]
    inline_string: Option<String>,
    #[serde(default, alias = "environmentVariable")]
    environment_variable: Option<String>,
    #[serde(default, alias = "watchedDirectory")]
    watched_directory: Option<WatchedDirectoryDto>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct WatchedDirectoryDto {
    #[serde(default)]
    path: String,
    #[serde(default, alias = "watchModify")]
    watch_modify: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RemoteDataSourceDto {
    #[serde(alias = "httpUri")]
    http_uri: HttpUriDto,
    #[serde(default)]
    sha256: String,
    #[serde(default, alias = "retryPolicy")]
    retry_policy: Option<RetryPolicyDto>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct HttpUriDto {
    uri: String,
    #[serde(default)]
    cluster: Option<String>,
    #[serde(default)]
    timeout: Option<ProtoDuration>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RetryPolicyDto {
    #[serde(default, alias = "retryBackOff")]
    retry_back_off: Option<BackoffStrategyDto>,
    #[serde(default, alias = "numRetries", alias = "max_retries", alias = "maxRetries")]
    num_retries: Option<u32>,
    #[serde(default, alias = "retryOn")]
    retry_on: Option<String>,
    #[serde(default, alias = "retryPriority")]
    retry_priority: Option<Value>,
    #[serde(default, alias = "retryHostPredicate")]
    retry_host_predicate: Vec<Value>,
    #[serde(default, alias = "hostSelectionRetryMaxAttempts")]
    host_selection_retry_max_attempts: i64,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct BackoffStrategyDto {
    #[serde(default, alias = "baseInterval")]
    base_interval: Option<ProtoDuration>,
    #[serde(default, alias = "maxInterval")]
    max_interval: Option<ProtoDuration>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum ProtoDuration {
    String(String),
    Object(ProtoDurationObject),
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProtoDurationObject {
    #[serde(default, deserialize_with = "deserialize_i64_string_or_number")]
    seconds: i64,
    #[serde(default)]
    nanos: i32,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum I64StringOrNumber {
    String(String),
    Number(i64),
}

fn deserialize_i64_string_or_number<'de, D>(deserializer: D) -> Result<i64, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = I64StringOrNumber::deserialize(deserializer)?;
    match value {
        I64StringOrNumber::Number(value) => Ok(value),
        I64StringOrNumber::String(value) => value.parse().map_err(serde::de::Error::custom),
    }
}

fn convert_plugin(plugin: PluginConfigDto, report: &mut ReportBuilder) -> Option<WasmConfig> {
    if !plugin.name.is_empty() {
        report.blocker(
            "plugin_identity_unsupported",
            "/name",
            None,
            "Envoy exposes name to the guest as plugin_name, but Edgion fixes that property to Wasm; a resource name cannot preserve it.",
            "Use an empty Envoy PluginConfig name only after confirming the guest does not require a custom plugin identity.",
        );
    } else {
        report.warning(
            "plugin_identity_default_differs",
            "/name",
            None,
            "Envoy exposes an empty plugin_name to the guest, while Edgion fixes that property to Wasm.",
            "Test that the guest does not depend on plugin_name.",
        );
    }
    block_nonempty_id(report, "/root_id", &plugin.root_id, "root_id");
    if let Some(capabilities) = &plugin.capability_restriction_config {
        let _ = capabilities.allowed_capabilities.len();
        report.blocker(
            "capability_restrictions_unsupported",
            "/capability_restriction_config",
            None,
            "Envoy capability restrictions cannot be preserved, including an empty map.",
            "Review the guest against Edgion's fixed host ABI and remove this field.",
        );
    }
    if let Some(reload) = &plugin.reload_config {
        let _ = reload.backoff.is_some();
        report.blocker(
            "reload_config_unsupported",
            "/reload_config",
            None,
            "Envoy VM reload configuration cannot be preserved.",
            "Remove reload_config and choose an Edgion-compatible failure policy.",
        );
    }
    if plugin.allow_on_headers_stop_iteration.is_some() {
        report.blocker(
            "headers_stop_iteration_unsupported",
            "/allow_on_headers_stop_iteration",
            None,
            "Envoy header stop-iteration behavior cannot be preserved.",
            "Remove allow_on_headers_stop_iteration and review guest pause behavior.",
        );
    }

    let fail_open = convert_failure_policy(&plugin, report);
    let plugin_config = convert_any(plugin.configuration.as_ref(), "/configuration", "/pluginConfig", report);
    let Some(vm) = plugin.vm_config else {
        report.blocker(
            "missing_vm_config",
            "/vm_config",
            None,
            "PluginConfig has no inline VmConfig.",
            "Provide vm_config with one supported code source.",
        );
        return None;
    };
    block_nonempty_id(report, "/vm_config/vm_id", &vm.vm_id, "vm_id");
    convert_runtime(&vm.runtime, report);
    if vm.allow_precompiled {
        report.blocker(
            "allow_precompiled_unsupported",
            "/vm_config/allow_precompiled",
            None,
            "Precompiled Wasm code is not accepted by the converter.",
            "Use portable Wasm bytecode and remove allow_precompiled.",
        );
    }
    if vm.nack_on_code_cache_miss {
        report.blocker(
            "nack_on_cache_miss_unsupported",
            "/vm_config/nack_on_code_cache_miss",
            None,
            "Envoy code-cache NACK timing cannot be preserved.",
            "Remove nack_on_code_cache_miss and review Edgion module loading behavior.",
        );
    }
    if let Some(environment) = &vm.environment_variables {
        // Do not inspect or report keys/values. Presence alone is a blocker.
        let _ = (environment.host_env_keys.len(), environment.key_values.len());
        report.blocker(
            "environment_variables_unsupported",
            "/vm_config/environment_variables",
            None,
            "Envoy VM environment injection cannot be preserved.",
            "Remove environment_variables and pass non-secret data through explicit guest configuration.",
        );
    }
    let vm_config = convert_any(
        vm.configuration.as_ref(),
        "/vm_config/configuration",
        "/vmConfig",
        report,
    );
    let Some(code) = vm.code.as_ref() else {
        report.blocker(
            "missing_code",
            "/vm_config/code",
            Some("/source"),
            "VmConfig has no Wasm code source.",
            "Provide exactly one supported local or remote code source.",
        );
        return None;
    };
    let source = convert_source(code, report);
    let (source, sha256, fetch) = source?;

    report.warning(
        "failure_semantics_differ",
        "/failure_policy",
        Some("/failOpen"),
        "Edgion fail-closed HTTP status and module startup timing differ from Envoy.",
        "Test load failures, traps, and startup behavior before deployment.",
    );
    report.warning(
        "callouts_disabled_by_default",
        "/",
        Some("/calloutAllowlist"),
        "Generated calloutAllowlist is empty, so guest HTTP callouts are disabled.",
        "Add reviewed exact origins only if the guest requires callouts.",
    );
    report.warning(
        "edgion_runtime_defaults",
        "/",
        Some("/timeoutMs"),
        "Edgion hook timeout and instance-pool defaults have no direct Envoy equivalents.",
        "Review timeoutMs, instancePoolSize, and calloutTimeoutMs.",
    );

    Some(WasmConfig {
        source: WasmSourceSpec { fetch, ..source },
        sha256,
        plugin_config,
        vm_config,
        fail_open,
        ..Default::default()
    })
}

fn block_nonempty_id(report: &mut ReportBuilder, path: &str, value: &str, field: &str) {
    if !value.is_empty() {
        report.blocker(
            "context_identity_unsupported",
            path,
            None,
            &format!("Non-empty Envoy {field} context identity cannot be preserved."),
            &format!("Clear {field} only after confirming the guest does not depend on context sharing."),
        );
    }
}

fn convert_runtime(runtime: &str, report: &mut ReportBuilder) {
    if runtime.is_empty() {
        report.warning(
            "runtime_defaulted_to_wasmtime",
            "/vm_config/runtime",
            None,
            "Envoy runtime selection is empty; Edgion always uses Wasmtime.",
            "Test the module on Edgion's Wasmtime host profile.",
        );
    } else if runtime != ENVOY_WASMTIME_RUNTIME {
        report.blocker(
            "runtime_unsupported",
            "/vm_config/runtime",
            None,
            "The selected Envoy runtime is not Edgion's exact Wasmtime runtime.",
            "Rebuild and test the guest for envoy.wasm.runtime.wasmtime.",
        );
    }
}

fn convert_failure_policy(plugin: &PluginConfigDto, report: &mut ReportBuilder) -> bool {
    let Some(policy) = plugin.failure_policy.as_deref() else {
        return plugin.fail_open;
    };
    if plugin.fail_open && policy != "UNSPECIFIED" {
        report.blocker(
            "failure_policy_conflict",
            "/failure_policy",
            Some("/failOpen"),
            "Deprecated fail_open conflicts with a non-UNSPECIFIED failure_policy.",
            "Keep fail_open only with an absent or UNSPECIFIED failure_policy.",
        );
    }
    match policy {
        "UNSPECIFIED" => plugin.fail_open,
        "FAIL_CLOSED" => false,
        "FAIL_OPEN" => !plugin.fail_open,
        "FAIL_RELOAD" => {
            report.blocker(
                "fail_reload_unsupported",
                "/failure_policy",
                None,
                "FAIL_RELOAD cannot be represented by Edgion's failOpen boolean.",
                "Choose FAIL_CLOSED or FAIL_OPEN after reviewing the safety impact.",
            );
            false
        }
        _ => {
            report.blocker(
                "failure_policy_unknown",
                "/failure_policy",
                None,
                "The Envoy failure policy enum value is unknown.",
                "Use an official v3 FailurePolicy value.",
            );
            false
        }
    }
}

fn convert_any(
    value: Option<&Value>,
    source_path: &str,
    target_path: &str,
    report: &mut ReportBuilder,
) -> Option<WasmRawConfig> {
    let value = value?;
    let Some(object) = value.as_object() else {
        report.blocker(
            "any_shape_unsupported",
            source_path,
            Some(target_path),
            "Envoy Any must use the protobuf JSON object representation.",
            "Use an empty Any, google.protobuf.StringValue, or google.protobuf.BytesValue.",
        );
        return None;
    };
    if object.is_empty() {
        return None;
    }
    let allowed = ["@type", "type_url", "typeUrl", "value"];
    if object.keys().any(|key| !allowed.contains(&key.as_str())) {
        report.blocker(
            "arbitrary_any_unsupported",
            source_path,
            Some(target_path),
            "Struct and arbitrary Any payloads are blocked because exact guest bytes cannot be proven.",
            "Encode the exact bytes as google.protobuf.StringValue or BytesValue.",
        );
        return None;
    }
    let type_url_keys = ["@type", "type_url", "typeUrl"]
        .into_iter()
        .filter(|key| object.contains_key(*key))
        .collect::<Vec<_>>();
    if type_url_keys.len() > 1 {
        report.blocker(
            "any_type_conflict",
            source_path,
            Some(target_path),
            "Non-empty Envoy Any sets more than one type URL alias.",
            "Keep exactly one of @type, type_url, or typeUrl.",
        );
        return None;
    }
    let type_url = type_url_keys
        .first()
        .and_then(|key| object.get(*key))
        .and_then(Value::as_str);
    let Some(type_url) = type_url else {
        report.blocker(
            "any_type_missing",
            source_path,
            Some(target_path),
            "Non-empty Envoy Any has no type URL.",
            "Use a supported wrapper Any with @type.",
        );
        return None;
    };
    match type_url {
        "type.googleapis.com/google.protobuf.StringValue" => match object.get("value").and_then(Value::as_str) {
            Some(raw_value) => checked_raw_config(WasmRawConfig::string(raw_value), source_path, target_path, report),
            None => {
                report.blocker(
                    "any_value_invalid",
                    source_path,
                    Some(target_path),
                    "StringValue requires a string value.",
                    "Set value using the wrapper's protobuf JSON representation.",
                );
                None
            }
        },
        "type.googleapis.com/google.protobuf.BytesValue" => {
            let Some(raw_value) = object.get("value").and_then(Value::as_str) else {
                report.blocker(
                    "any_value_invalid",
                    source_path,
                    Some(target_path),
                    "BytesValue requires a base64 string value.",
                    "Set value using the wrapper's protobuf JSON representation.",
                );
                return None;
            };
            match decode_proto_json_base64(raw_value) {
                Some(bytes) => checked_raw_config(
                    WasmRawConfig::base64(base64::engine::general_purpose::STANDARD.encode(bytes)),
                    source_path,
                    target_path,
                    report,
                ),
                None => {
                    report.blocker(
                        "bytes_value_invalid",
                        source_path,
                        Some(target_path),
                        "BytesValue is not valid protobuf JSON base64.",
                        "Encode the exact bytes with a protobuf JSON base64 spelling.",
                    );
                    None
                }
            }
        }
        "type.googleapis.com/google.protobuf.Struct" => {
            report.blocker(
                "struct_any_unsupported",
                source_path,
                Some(target_path),
                "google.protobuf.Struct conversion is blocked because Envoy's effective guest bytes are not stable enough for v1.",
                "Encode the intended exact bytes as StringValue or BytesValue.",
            );
            None
        }
        _ => {
            report.blocker(
                "arbitrary_any_unsupported",
                source_path,
                Some(target_path),
                "Arbitrary Envoy Any payloads are not supported.",
                "Use an empty Any, google.protobuf.StringValue, or google.protobuf.BytesValue.",
            );
            None
        }
    }
}

fn checked_raw_config(
    config: WasmRawConfig,
    source_path: &str,
    target_path: &str,
    report: &mut ReportBuilder,
) -> Option<WasmRawConfig> {
    if config.guest_bytes().is_err() {
        report.blocker(
            "guest_configuration_too_large",
            source_path,
            Some(target_path),
            "Decoded guest configuration exceeds Edgion's 64KiB limit.",
            "Reduce the exact StringValue or BytesValue payload.",
        );
        None
    } else {
        Some(config)
    }
}

fn convert_source(
    code: &AsyncDataSourceDto,
    report: &mut ReportBuilder,
) -> Option<(WasmSourceSpec, Option<String>, Option<WasmFetchSpec>)> {
    let count = usize::from(code.local.is_some()) + usize::from(code.remote.is_some());
    if count != 1 {
        report.blocker(
            "code_source_oneof_conflict",
            "/vm_config/code",
            Some("/source"),
            "Envoy code must set exactly one of local or remote.",
            "Keep one code source.",
        );
        return None;
    }
    if let Some(local) = &code.local {
        convert_local_source(local, report)
    } else {
        code.remote
            .as_ref()
            .and_then(|remote| convert_remote_source(remote, report))
    }
}

fn convert_local_source(
    local: &LocalDataSourceDto,
    report: &mut ReportBuilder,
) -> Option<(WasmSourceSpec, Option<String>, Option<WasmFetchSpec>)> {
    let count = usize::from(local.filename.is_some())
        + usize::from(local.inline_bytes.is_some())
        + usize::from(local.inline_string.is_some())
        + usize::from(local.environment_variable.is_some());
    if count != 1 {
        report.blocker(
            "local_source_oneof_conflict",
            "/vm_config/code/local",
            Some("/source"),
            "Envoy local code must set exactly one DataSource specifier.",
            "Keep one of filename, inline_bytes, or inline_string.",
        );
        return None;
    }
    if local.environment_variable.is_some() {
        report.blocker(
            "local_environment_variable_unsupported",
            "/vm_config/code/local/environment_variable",
            None,
            "Reading Wasm code from a local environment variable is blocked.",
            "Use filename, inline_bytes, or inline_string.",
        );
    }
    if let Some(watched) = &local.watched_directory {
        let _ = (&watched.path, watched.watch_modify);
        report.blocker(
            "watched_directory_unsupported",
            "/vm_config/code/local/watched_directory",
            None,
            "Envoy watched-directory reload semantics cannot be preserved.",
            "Remove watched_directory and use Edgion resource hot reload explicitly.",
        );
    }
    if let Some(filename) = &local.filename {
        if filename.is_empty() {
            report.blocker(
                "local_filename_empty",
                "/vm_config/code/local/filename",
                Some("/source/file"),
                "The local filename is empty.",
                "Provide a non-empty module path.",
            );
        }
        if Path::new(filename).is_relative() {
            report.warning(
                "relative_file_semantics",
                "/vm_config/code/local/filename",
                Some("/source/file"),
                "Relative file paths resolve against the Edgion gateway work directory.",
                "Verify the deployed work directory or use an absolute mounted path.",
            );
        }
        return Some((
            WasmSourceSpec {
                file: Some(filename.clone()),
                ..Default::default()
            },
            None,
            None,
        ));
    }
    let bytes = if let Some(encoded) = &local.inline_bytes {
        match decode_proto_json_base64(encoded) {
            Some(bytes) => bytes,
            None => {
                report.blocker(
                    "inline_bytes_invalid",
                    "/vm_config/code/local/inline_bytes",
                    Some("/source/inline"),
                    "inline_bytes is not valid protobuf JSON base64.",
                    "Use a standard or URL-safe protobuf JSON base64 spelling.",
                );
                return None;
            }
        }
    } else if let Some(value) = &local.inline_string {
        value.as_bytes().to_vec()
    } else {
        return None;
    };
    if bytes.len() > MAX_INLINE_DECODED_BYTES {
        report.blocker(
            "inline_module_too_large",
            "/vm_config/code/local",
            Some("/source/inline"),
            "Inline Wasm code exceeds Edgion's 1MiB decoded limit.",
            "Use source.file or source.url for a larger module.",
        );
    }
    Some((
        WasmSourceSpec {
            inline: Some(base64::engine::general_purpose::STANDARD.encode(bytes)),
            ..Default::default()
        },
        None,
        None,
    ))
}

fn convert_remote_source(
    remote: &RemoteDataSourceDto,
    report: &mut ReportBuilder,
) -> Option<(WasmSourceSpec, Option<String>, Option<WasmFetchSpec>)> {
    if remote.sha256.is_empty() {
        report.blocker(
            "remote_sha256_required",
            "/vm_config/code/remote/sha256",
            Some("/sha256"),
            "Remote Envoy Wasm code has no mandatory SHA-256 digest.",
            "Set the 64-character hexadecimal digest.",
        );
    }
    match remote.http_uri.cluster.as_deref() {
        Some(cluster) if !cluster.is_empty() => {
            report.blocker(
                "remote_cluster_unsupported",
                "/vm_config/code/remote/http_uri/cluster",
                None,
                "Envoy cluster transport, TLS, authentication, DNS, and egress semantics cannot be recovered.",
                "Configure an audited Edgion URL source and fetch security manually.",
            );
        }
        Some(_) | None => {
            report.blocker(
                "remote_cluster_required",
                "/vm_config/code/remote/http_uri/cluster",
                None,
                "Envoy v3 HttpUri requires a non-empty cluster.",
                "Provide a valid Envoy cluster to audit; remote cluster semantics still require manual Edgion migration.",
            );
        }
    }
    validate_remote_uri(&remote.http_uri.uri, report);
    let timeout_ms = remote
        .http_uri
        .timeout
        .as_ref()
        .and_then(|duration| duration_ms(duration, "/vm_config/code/remote/http_uri/timeout", report));
    if let Some(timeout_ms) = timeout_ms {
        if timeout_ms == 0 || timeout_ms > 60_000 {
            report.blocker(
                "remote_timeout_out_of_range",
                "/vm_config/code/remote/http_uri/timeout",
                Some("/source/fetch/timeoutMs"),
                "Converted timeout is outside Edgion's 1..=60000ms range.",
                "Use an Envoy timeout between 1 millisecond and 60 seconds.",
            );
        }
    }
    if remote.http_uri.timeout.is_none() {
        report.blocker(
            "remote_timeout_required",
            "/vm_config/code/remote/http_uri/timeout",
            Some("/source/fetch/timeoutMs"),
            "Envoy v3 HttpUri requires a positive timeout.",
            "Set an explicit timeout greater than zero.",
        );
    }
    let retry = convert_retry(remote.retry_policy.as_ref(), report);
    report.warning(
        "remote_retry_semantics_differ",
        "/vm_config/code/remote/retry_policy",
        Some("/source/fetch/retry"),
        "Retry attempts and base backoff are mapped, but Envoy and Edgion retry scheduling details differ.",
        "Test artifact-server failure and recovery timing before deployment.",
    );
    Some((
        WasmSourceSpec {
            url: Some(remote.http_uri.uri.clone()),
            ..Default::default()
        },
        if remote.sha256.is_empty() {
            None
        } else {
            Some(remote.sha256.clone())
        },
        Some(WasmFetchSpec {
            timeout_ms: timeout_ms.unwrap_or(10_000),
            retry: Some(retry),
            ..Default::default()
        }),
    ))
}

fn validate_remote_uri(uri: &str, report: &mut ReportBuilder) {
    let Ok(parsed) = Url::parse(uri) else {
        report.blocker(
            "remote_uri_invalid",
            "/vm_config/code/remote/http_uri/uri",
            Some("/source/url"),
            "Remote URI is not a valid absolute URL.",
            "Use an absolute HTTP(S) URL without userinfo.",
        );
        return;
    };
    if !matches!(parsed.scheme(), "http" | "https") {
        report.blocker(
            "remote_uri_scheme_unsupported",
            "/vm_config/code/remote/http_uri/uri",
            Some("/source/url"),
            "Remote URI is not HTTP or HTTPS.",
            "Use an HTTP(S) artifact URL.",
        );
    }
    if parsed.host_str().is_none() {
        report.blocker(
            "remote_uri_invalid",
            "/vm_config/code/remote/http_uri/uri",
            Some("/source/url"),
            "Remote HTTP(S) URI has no host.",
            "Use a full absolute artifact URL.",
        );
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        report.blocker(
            "remote_uri_userinfo_forbidden",
            "/vm_config/code/remote/http_uri/uri",
            Some("/source/url"),
            "Remote URI userinfo is forbidden and has not been included in this report.",
            "Remove credentials from the URL and configure an Edgion Secret reference manually.",
        );
    }
}

fn decode_proto_json_base64(value: &str) -> Option<Vec<u8>> {
    [
        &base64::engine::general_purpose::STANDARD,
        &base64::engine::general_purpose::STANDARD_NO_PAD,
        &base64::engine::general_purpose::URL_SAFE,
        &base64::engine::general_purpose::URL_SAFE_NO_PAD,
    ]
    .into_iter()
    .find_map(|engine| engine.decode(value).ok())
}

fn convert_retry(retry: Option<&RetryPolicyDto>, report: &mut ReportBuilder) -> WasmFetchRetry {
    let mut attempts = 2_u32;
    let mut backoff_ms = 1_000_u64;
    if let Some(retry) = retry {
        if let Some(num_retries) = retry.num_retries {
            match num_retries.checked_add(1) {
                Some(value) => attempts = value,
                None => report.blocker(
                    "retry_count_out_of_range",
                    "/vm_config/code/remote/retry_policy/num_retries",
                    Some("/source/fetch/retry/attempts"),
                    "Envoy retry count cannot be represented.",
                    "Use at most 9 retries (10 total attempts).",
                ),
            }
        }
        if retry.retry_on.as_deref().is_some_and(|value| !value.is_empty())
            || retry.retry_priority.is_some()
            || !retry.retry_host_predicate.is_empty()
            || retry.host_selection_retry_max_attempts != 0
        {
            report.blocker(
                "advanced_retry_semantics_unsupported",
                "/vm_config/code/remote/retry_policy",
                None,
                "Envoy advanced retry selection semantics cannot be preserved.",
                "Reduce the policy to retry_back_off and num_retries.",
            );
        }
        if let Some(backoff) = &retry.retry_back_off {
            if backoff.max_interval.is_some() {
                report.blocker(
                    "retry_max_interval_unsupported",
                    "/vm_config/code/remote/retry_policy/retry_back_off/max_interval",
                    None,
                    "Envoy maximum retry interval cannot be preserved.",
                    "Remove max_interval after reviewing Edgion's fixed exponential retry behavior.",
                );
            }
            if let Some(base) = &backoff.base_interval {
                if let Some(value) = duration_ms(
                    base,
                    "/vm_config/code/remote/retry_policy/retry_back_off/base_interval",
                    report,
                ) {
                    backoff_ms = value;
                }
            }
        }
    }
    if attempts == 0 || attempts > 10 {
        report.blocker(
            "retry_count_out_of_range",
            "/vm_config/code/remote/retry_policy/num_retries",
            Some("/source/fetch/retry/attempts"),
            "Converted total attempts are outside Edgion's 1..=10 range.",
            "Use at most 9 Envoy retries.",
        );
    }
    if backoff_ms == 0 || backoff_ms > 30_000 {
        report.blocker(
            "retry_backoff_out_of_range",
            "/vm_config/code/remote/retry_policy/retry_back_off/base_interval",
            Some("/source/fetch/retry/backoffMs"),
            "Converted retry backoff is outside the supported 1..=30000ms range.",
            "Use a base interval from 1 millisecond through 30 seconds.",
        );
    }
    WasmFetchRetry { attempts, backoff_ms }
}

fn duration_ms(duration: &ProtoDuration, path: &str, report: &mut ReportBuilder) -> Option<u64> {
    let nanos = match duration {
        ProtoDuration::Object(value) => {
            if value.seconds < 0 || !(0..1_000_000_000).contains(&value.nanos) {
                None
            } else {
                (value.seconds as u128)
                    .checked_mul(1_000_000_000)
                    .and_then(|base| base.checked_add(value.nanos as u128))
            }
        }
        ProtoDuration::String(value) => parse_duration_nanos(value),
    };
    let Some(nanos) = nanos else {
        report.blocker(
            "duration_invalid",
            path,
            None,
            "Envoy duration is invalid or out of range.",
            "Use a non-negative protobuf duration with millisecond precision.",
        );
        return None;
    };
    if nanos % 1_000_000 != 0 {
        report.blocker(
            "duration_precision_unsupported",
            path,
            None,
            "Sub-millisecond Envoy duration precision cannot be preserved.",
            "Use an exact whole-millisecond duration.",
        );
        return None;
    }
    u64::try_from(nanos / 1_000_000).ok().or_else(|| {
        report.blocker(
            "duration_out_of_range",
            path,
            None,
            "Envoy duration is too large to represent in milliseconds.",
            "Use a smaller duration.",
        );
        None
    })
}

fn parse_duration_nanos(value: &str) -> Option<u128> {
    let body = value.strip_suffix('s')?;
    if body.starts_with('-') || body.is_empty() {
        return None;
    }
    let (seconds, fraction) = body.split_once('.').unwrap_or((body, ""));
    let seconds: u128 = seconds.parse().ok()?;
    if fraction.len() > 9 || !fraction.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let fraction_value: u128 = if fraction.is_empty() {
        0
    } else {
        fraction.parse::<u128>().ok()? * 10_u128.pow((9 - fraction.len()) as u32)
    };
    seconds.checked_mul(1_000_000_000)?.checked_add(fraction_value)
}

#[cfg(test)]
#[path = "../../../tests/unit/cli/commands/wasm_test.rs"]
mod tests;
