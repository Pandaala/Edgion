#!/usr/bin/env bash
# Forbid references to legacy types/kinds/fields removed by the EdgionData
# overlay refactor (commit 79b7de8e..a390b2d8 on refactor/edgion-data-overlay).
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
#   tasks/todo/edgion-data-overlay-refactor/06-decisions.md  -> D1, D4
#   tasks/todo/edgion-data-overlay-refactor/05-issues.md     -> I3
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

# 1. Removed kind: PluginMetaData (renamed to EdgionData in S1).
check "Legacy kind 'PluginMetaData' (use 'EdgionData')" \
    'kind:[[:space:]]*PluginMetaData([[:space:]]|$)' \
    --include='*.yaml' --include='*.yml'

# 2. Removed DataEntry variants (ClusterRegionRoute / ServiceRegionRoute / standalone
#    GlobalConnectionIpRestriction) -- removed in S3 and I3.
check "Legacy DataEntry 'type: ClusterRegionRoute' (folded into EdgionPlugins RegionRouteConfig base, S3)" \
    'type:[[:space:]]*ClusterRegionRoute([[:space:]]|$)' \
    --include='*.yaml' --include='*.yml'

check "Legacy DataEntry 'type: ServiceRegionRoute' (folded into RegionRouteOverride overlay, S3)" \
    'type:[[:space:]]*ServiceRegionRoute([[:space:]]|$)' \
    --include='*.yaml' --include='*.yml'

# Note: the discriminator 'type: GlobalConnectionIpRestriction' is STILL VALID
# inside EdgionStreamPlugins entries (it is the stream-plugin tag, not a
# DataEntry variant). We only forbid it as a DataEntry variant under
# `kind: EdgionData` / `data:` — caught by the parity gate (Gate 2), not here.

# 3. Removed spec fields.
check "Legacy field 'serviceRegionRouteRef' (use 'overrideRef' on RegionRouteConfig, S3)" \
    'serviceRegionRouteRef[[:space:]]*:' \
    --include='*.yaml' --include='*.yml' --include='*.rs'

check "Legacy field 'clusterRef' from the removed cluster_ref chain (S3)" \
    '^[[:space:]]*clusterRef[[:space:]]*:' \
    --include='*.yaml' --include='*.yml'

# 4. Removed Rust identifiers.
check "Legacy Rust kind 'PluginMetaData' / spec / status (use 'EdgionData' / 'EdgionDataSpec' / 'EdgionDataStatus')" \
    '\bPluginMetaData(Spec|Status)?\b' \
    --include='*.rs'

check "Legacy Rust enum 'MetaDataEntry' (use 'DataEntry')" \
    '\bMetaDataEntry\b' \
    --include='*.rs'

# Note: `PluginReference` is NOT forbidden -- there are two unrelated structs
# with that name. The PluginMetaData-side one was renamed to `EdgionDataRef`.
# The other (in edgion-resources/.../edgion_gateway_config.rs) is for EdgionPlugins
# global-plugin refs and is unrelated to the EdgionData refactor.
check "Legacy Rust type 'PluginMetaDataRef' / 'DataRef' (use 'EdgionDataRef')" \
    '\b(PluginMetaDataRef|DataRef)\b' \
    --include='*.rs'

check "Legacy Rust type 'PluginMetaDataStore' / 'DataHandler' / 'PluginMetaDataRefManager' (use Edgion-prefixed)" \
    '\b(PluginMetaDataStore|DataHandler|PluginMetaDataRefManager)\b' \
    --include='*.rs'

# 4b. Module path naming: bare 'data'/'plugin_metadata_*' under Edgion crates
#     violates the project's `edgion_<name>` convention (matching
#     edgion_acme/edgion_plugins/edgion_stream_plugins/edgion_tls).
check "Legacy module path 'plugin_metadata_utils' (use 'edgion_data_utils')" \
    'plugin_metadata_utils' \
    --include='*.rs'

check "Legacy bare-name 'mod data;' under resources/ runtime/ handlers/ (use 'mod edgion_data;')" \
    '^(pub )?mod data;' \
    --include='*.rs'

check "Legacy bare path '::data::' (use '::edgion_data::')" \
    '::data::' \
    --include='*.rs'

check "Legacy field/var 'plugin_metadata_refs' / 'pm_key' / 'pm_ns' / 'pm_name' / 'remove_pm' / 'requeue_pm_referrers' (use ed_*/edgion_data_*)" \
    '\b(plugin_metadata_refs|pm_key|pm_ns|pm_name|remove_pm|requeue_pm_referrers)\b' \
    --include='*.rs'

check "Legacy DataEntry::ClusterRegionRoute / ::ServiceRegionRoute / standalone ::GlobalConnectionIpRestriction" \
    '\bDataEntry::(ClusterRegionRoute|ServiceRegionRoute|GlobalConnectionIpRestriction)\b' \
    --include='*.rs'

# 5. Removed CRD file (the old kebab-name).
if [ -f "config/crd/edgion-crd/plugin_metadata_crd.yaml" ]; then
    echo "ERROR: Legacy CRD file 'config/crd/edgion-crd/plugin_metadata_crd.yaml' still exists (use 'edgion_data_crd.yaml')" >&2
    violations=$((violations + 1))
fi

if [ "$violations" -gt 0 ]; then
    echo "Found $violations class(es) of legacy data references. See above." >&2
    exit 1
fi
echo "OK: no legacy PluginMetaData / ClusterRegionRoute / ServiceRegionRoute / *Ref references in code or fixtures"
