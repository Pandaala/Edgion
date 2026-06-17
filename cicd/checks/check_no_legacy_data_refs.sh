#!/usr/bin/env bash
# Forbid references to legacy types/kinds/fields removed by the EdgionConfigData
# overlay refactor (commit 79b7de8e..a390b2d8 on refactor/edgion-config-data-overlay),
# AND the now-legacy intermediate name 'EdgionData'/'DataEntry' (renamed to
# 'EdgionConfigData'/'ConfigEntry' on 2026-06-16 -- see section 6 below). The full
# rename chain of this resource is: PluginMetaData -> EdgionData -> EdgionConfigData.
#
# Catches a class of regression where someone copies an old example, fixture, or
# CRD snippet back in, or where a new branch built on a stale checkpoint
# reintroduces the removed names. Each forbidden token is grepped over its
# applicable file types (Rust source for type identifiers; YAML for kinds /
# CRD `type:` discriminators / spec fields).
#
# Allowed locations (excluded from the scan):
#   - tasks/                  (planning + history docs intentionally reference
#                              the legacy names, e.g. 02-inventory.md
#                              "Authoritative rename map", D-S* deviation logs)
#   - cicd/checks/            (this script defines the forbidden strings)
#   - target/                 (build artifacts)
#   - .git/                   (history)
#   - 05-issues.md            (issues file by definition references closed-state names)
#
# Policy source:
#   tasks/todo/edgion-config-data-overlay-refactor/06-decisions.md  -> D1, D4
#   tasks/todo/edgion-config-data-overlay-refactor/05-issues.md     -> I3
#
# Exit 0 on clean; exit 1 with violation list otherwise.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../.." && pwd)"
cd "${REPO_ROOT}"

# Common exclude paths for any grep below.
# - tasks/, cicd/: planning docs + this script intentionally reference legacy names.
# - tmp/: integration_testing/ run artifacts copy historical CRDs; .gitignored.
# - target/, .git/, node_modules/: build artifacts / SCM internals.
EXCLUDES=(
    --exclude-dir=tasks
    --exclude-dir=cicd
    --exclude-dir=tmp
    --exclude-dir=target
    --exclude-dir=.git
    --exclude-dir=node_modules
)

violations=0

check() {
    local label="$1"
    local pattern="$2"
    shift 2
    local hits
    if hits=$(grep -rnE "$pattern" "${EXCLUDES[@]}" "$@" 2>/dev/null); then
        echo "ERROR: $label" >&2
        echo "$hits" >&2
        echo >&2
        violations=$((violations + 1))
    fi
}

# 1. Removed kind: PluginMetaData (renamed to EdgionConfigData in S1).
check "Legacy kind 'PluginMetaData' (use 'EdgionConfigData')" \
    'kind:[[:space:]]*PluginMetaData([[:space:]]|$)' \
    --include='*.yaml' --include='*.yml'

# 2. Removed ConfigEntry variants (ClusterRegionRoute / ServiceRegionRoute / standalone
#    GlobalConnectionIpRestriction) -- removed in S3 and I3.
check "Legacy ConfigEntry 'type: ClusterRegionRoute' (folded into EdgionPlugins RegionRouteConfig base, S3)" \
    'type:[[:space:]]*ClusterRegionRoute([[:space:]]|$)' \
    --include='*.yaml' --include='*.yml'

check "Legacy ConfigEntry 'type: ServiceRegionRoute' (folded into RegionRouteOverride overlay, S3)" \
    'type:[[:space:]]*ServiceRegionRoute([[:space:]]|$)' \
    --include='*.yaml' --include='*.yml'

# Note: the discriminator 'type: GlobalConnectionIpRestriction' is STILL VALID
# inside EdgionStreamPlugins entries (it is the stream-plugin tag, not a
# ConfigEntry variant). We only forbid it as a ConfigEntry variant under
# `kind: EdgionConfigData` / `data:` — caught by the parity gate (Gate 2), not here.

# 3. Removed spec fields.
check "Legacy field 'serviceRegionRouteRef' (use 'overrideRef' on RegionRouteConfig, S3)" \
    'serviceRegionRouteRef[[:space:]]*:' \
    --include='*.yaml' --include='*.yml' --include='*.rs'

check "Legacy field 'clusterRef' from the removed cluster_ref chain (S3)" \
    '^[[:space:]]*clusterRef[[:space:]]*:' \
    --include='*.yaml' --include='*.yml'

# 4. Removed Rust identifiers.
check "Legacy Rust kind 'PluginMetaData' / spec / status (use 'EdgionConfigData' / 'EdgionConfigDataSpec' / 'EdgionConfigDataStatus')" \
    '\bPluginMetaData(Spec|Status)?\b' \
    --include='*.rs'

check "Legacy Rust enum 'MetaConfigEntry' (use 'ConfigEntry')" \
    '\bMetaConfigEntry\b' \
    --include='*.rs'

# Note: `PluginReference` is NOT forbidden -- there are two unrelated structs
# with that name. The PluginMetaData-side one was renamed to `EdgionConfigDataRef`.
# The other (in edgion-resources/.../edgion_gateway_config.rs) is for EdgionPlugins
# global-plugin refs and is unrelated to the EdgionConfigData refactor.
check "Legacy Rust type 'PluginMetaDataRef' / 'DataRef' (use 'EdgionConfigDataRef')" \
    '\b(PluginMetaDataRef|DataRef)\b' \
    --include='*.rs'

check "Legacy Rust type 'PluginMetaDataStore' / 'DataHandler' / 'PluginMetaDataRefManager' (use Edgion-prefixed)" \
    '\b(PluginMetaDataStore|DataHandler|PluginMetaDataRefManager)\b' \
    --include='*.rs'

# 4b. Module path naming: bare 'data'/'plugin_metadata_*' under Edgion crates
#     violates the project's `edgion_<name>` convention (matching
#     edgion_acme/edgion_plugins/edgion_stream_plugins/edgion_tls).
check "Legacy module path 'plugin_metadata_utils' (use 'edgion_config_data_utils')" \
    'plugin_metadata_utils' \
    --include='*.rs'

check "Legacy bare-name 'mod data;' under resources/ runtime/ handlers/ (use 'mod edgion_config_data;')" \
    '^(pub )?mod data;' \
    --include='*.rs'

check "Legacy bare path '::data::' (use '::edgion_config_data::')" \
    '::data::' \
    --include='*.rs'

check "Legacy field/var 'plugin_metadata_refs' / 'pm_key' / 'pm_ns' / 'pm_name' / 'remove_pm' / 'requeue_pm_referrers' (use ed_*/edgion_config_data_*)" \
    '\b(plugin_metadata_refs|pm_key|pm_ns|pm_name|remove_pm|requeue_pm_referrers)\b' \
    --include='*.rs'

# 5. Test suite naming -- the integration test scaffolding got renamed alongside the
#    data refactor so AI agents don't search the old names by accident.
check "Legacy test module 'plugin_metadata_status' / 'plugin_metadata_managed' / 'pmd_ref_flap' (use 'edgion_config_data_*')" \
    'plugin_metadata_(status|managed)|pmd_ref_flap' \
    --include='*.rs'

check "Legacy test struct 'PmdFlap*TestSuite' (use 'EdgionConfigDataFlap*TestSuite')" \
    'PmdFlap[A-Z]\w*TestSuite' \
    --include='*.rs'

check "Legacy kebab fixture dir 'plugin-metadata-managed' / 'pmd-flap-*' name prefix (use 'EdgionConfigDataManaged' / 'data-flap-*')" \
    'plugin-metadata-managed|pmd-flap-' \
    --include='*.yaml' --include='*.yml' --include='*.rs'

check "Legacy resource-name prefix 'pm-managed' / 'pm-validated' / 'pm-status-' / 'esp-gcir-pm-' (use 'data-*' / 'esp-gcir-data-')" \
    '\bpm-(managed|validated|status-)|esp-gcir-pm-' \
    --include='*.yaml' --include='*.yml' --include='*.rs'

check "Legacy fixture file-name component 'PluginMetaData_' (use 'EdgionConfigData_')" \
    'PluginMetaData_' \
    --include='*.yaml' --include='*.yml' --include='*.rs'

check "Legacy ConfigEntry::ClusterRegionRoute / ::ServiceRegionRoute / standalone ::GlobalConnectionIpRestriction" \
    '\bConfigEntry::(ClusterRegionRoute|ServiceRegionRoute|GlobalConnectionIpRestriction)\b' \
    --include='*.rs'

# Removed in the GCIR selector refactor (2026-06-16): the whole-profiles overlay
# `GlobalConnIpRestrictionOverride` + `merge_gcir_override`, and the old base+overrideRef wrapper
# `GlobalConnectionIpRestrictionEntry` / type `GlobalConnectionIpRestrictionData`. GlobalConnectionIp-
# Restriction now carries its config inline (`GlobalConnectionIpRestrictionConfig`) and consumes a
# `Selector` EdgionConfigData via `activeProfileRef`.
check "Removed GCIR overlay symbols 'GlobalConnIpRestrictionOverride' / 'merge_gcir_override' (use 'Selector' + activeProfileRef)" \
    '\b(GlobalConnIpRestrictionOverride|merge_gcir_override)\b' \
    --include='*.rs' --include='*.yaml' --include='*.yml'

check "Removed GCIR wrapper 'GlobalConnectionIpRestrictionEntry' / 'GlobalConnectionIpRestrictionData' (use 'GlobalConnectionIpRestrictionConfig')" \
    '\bGlobalConnectionIpRestriction(Entry|Data)\b' \
    --include='*.rs'

# 5. Removed CRD file (the old kebab-name).
if [ -f "config/crd/edgion-crd/plugin_metadata_crd.yaml" ]; then
    echo "ERROR: Legacy CRD file 'config/crd/edgion-crd/plugin_metadata_crd.yaml' still exists (use 'edgion_config_data_crd.yaml')" >&2
    violations=$((violations + 1))
fi
if [ -f "config/crd/edgion-crd/edgion_data_crd.yaml" ]; then
    echo "ERROR: Legacy CRD file 'config/crd/edgion-crd/edgion_data_crd.yaml' still exists (use 'edgion_config_data_crd.yaml')" >&2
    violations=$((violations + 1))
fi

# 6. Now-legacy INTERMEDIATE name 'EdgionData' / 'DataEntry' -- renamed to
#    'EdgionConfigData' / 'ConfigEntry' on 2026-06-16 (second rename of this resource;
#    chain: PluginMetaData -> EdgionData -> EdgionConfigData). None of these tokens
#    appear inside the new names (`EdgionConfigData`, `edgion_config_data`,
#    `edgionconfigdata`, `ConfigEntry`), so forbidding them is false-positive-free.
#    Mapping-definition docs that still reference 'EdgionData' live under tasks/ and
#    docs/ (.md), neither scanned by the .rs/.yaml includes below.
check "Legacy Rust identifier 'EdgionData*' (use 'EdgionConfigData*'; renamed 2026-06-16)" \
    '\bEdgionData[A-Za-z]*\b' \
    --include='*.rs'

check "Legacy Rust enum 'DataEntry' (use 'ConfigEntry'; renamed 2026-06-16)" \
    '\bDataEntry\b' \
    --include='*.rs'

check "Legacy SCREAMING_SNAKE 'EDGION_DATA_*' identifier (use 'EDGION_CONFIG_DATA_*')" \
    '\bEDGION_DATA_[A-Z]' \
    --include='*.rs'

check "Legacy kind 'EdgionData' (use 'EdgionConfigData'; renamed 2026-06-16)" \
    'kind:[[:space:]]*EdgionData([[:space:]]|$)' \
    --include='*.yaml' --include='*.yml'

check "Legacy lowercase resource/plural 'edgiondata' or 'pluginmetadata' in K8s manifests (use 'edgionconfigdata')" \
    '\b(edgiondata|pluginmetadata)\b' \
    --include='*.yaml' --include='*.yml'

if [ "$violations" -gt 0 ]; then
    echo "Found $violations class(es) of legacy data references. See above." >&2
    exit 1
fi
echo "OK: no legacy PluginMetaData / ClusterRegionRoute / ServiceRegionRoute / *Ref references in code or fixtures"
