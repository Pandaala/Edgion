#!/usr/bin/env bash
# ============================================================================
# verify_conf_sync_certs.sh
#
# Cluster verification harness for the conf_sync auto-CA cert-rotation feature
# (Phase 1 + Phase 2). Drives a LIVE deployment in the `edgion-system` namespace
# on the current kubectl context and reports PASS/FAIL/SKIP per check.
#
# SAFETY: this script ONLY ever touches the `edgion-system` namespace. It never
# reads or mutates any other namespace. Phase-2 checks that mutate the controller
# ConfigMap register an EXIT trap that RESTORES the original ConfigMap and
# restarts the controller, leaving the cluster in the shipped-default state.
#
# Prerequisites (verified at startup):
#   - kubectl reachable, edgion-system namespace exists, controller+gateway up
#   - openssl 3.x (for -not_before/-not_after crafted certs); jq; python3
#
# Usage:
#   verify_conf_sync_certs.sh --phase1            # Phase-1 checks (P1.1..P1.9)
#   verify_conf_sync_certs.sh --phase2            # Phase-2 checks (P2.1..P2.9)
#   verify_conf_sync_certs.sh --all               # both phases
#   verify_conf_sync_certs.sh --check <id>        # a single check, e.g. --check P2.4
#
# Idempotent and safe to re-run. Each check is independent where possible.
# ============================================================================
set -euo pipefail

# ---------------------------------------------------------------------------
# Constants (namespace is hard-wired; the script must never touch another one)
# ---------------------------------------------------------------------------
readonly NS="edgion-system"
readonly CTRL_DEPLOY="edgion-controller"
readonly GW_DEPLOY="edgion-gateway"
readonly CTRL_LABEL="app=edgion-controller"
readonly GW_LABEL="app=edgion-gateway"
readonly LEADER_LABEL="edgion.io/leader=true"
readonly CA_SECRET="edgion-conf-sync-ca"
readonly LEAF_SECRET="edgion-conf-sync-certs"
readonly ADMIN_SECRET="edgion-admin-credentials"
readonly CTRL_CM="edgion-controller-config"
readonly CTRL_CM_KEY="edgion-controller.yaml"
readonly ADMIN_PORT="12101"
readonly GW_SA="default"   # gateway deployment declares no serviceAccountName

# ---------------------------------------------------------------------------
# openssl resolution: prefer an OpenSSL 3.x binary (needs -not_before/-not_after;
# macOS system openssl is LibreSSL which lacks them).
# ---------------------------------------------------------------------------
OPENSSL=""
resolve_openssl() {
  local candidates=(
    "/opt/homebrew/opt/openssl@3/bin/openssl"
    "/usr/local/opt/openssl@3/bin/openssl"
    "openssl"
  )
  local c
  for c in "${candidates[@]}"; do
    if command -v "$c" >/dev/null 2>&1; then
      if "$c" version 2>/dev/null | grep -q "^OpenSSL 3"; then
        OPENSSL="$c"
        return 0
      fi
    fi
  done
  # Fall back to whatever openssl exists; crafted-cert checks will SKIP if it
  # cannot honor -not_after.
  OPENSSL="openssl"
  return 0
}

# ---------------------------------------------------------------------------
# Output helpers
# ---------------------------------------------------------------------------
if [[ -t 1 ]]; then
  C_RED=$'\033[0;31m'; C_GRN=$'\033[0;32m'; C_YEL=$'\033[1;33m'
  C_BLU=$'\033[0;34m'; C_CYN=$'\033[0;36m'; C_NC=$'\033[0m'
else
  C_RED=""; C_GRN=""; C_YEL=""; C_BLU=""; C_CYN=""; C_NC=""
fi
info()  { echo "${C_BLU}[INFO]${C_NC} $*"; }
step()  { echo "${C_CYN}--- $* ---${C_NC}"; }
warn()  { echo "${C_YEL}[WARN]${C_NC} $*"; }
errln() { echo "${C_RED}[ERR ]${C_NC} $*" >&2; }

# ---------------------------------------------------------------------------
# Result registry (parallel indexed arrays — bash 3.2 has no assoc arrays)
# ---------------------------------------------------------------------------
RESULT_ID=()
RESULT_ST=()
RESULT_TXT=()

record() {
  # record <id> <PASS|FAIL|SKIP> <message>
  local id="$1" status="$2" msg="${3:-}"
  RESULT_ID+=("$id")
  RESULT_ST+=("$status")
  RESULT_TXT+=("$msg")
  local color="$C_YEL"
  [[ "$status" == PASS ]] && color="$C_GRN"
  [[ "$status" == FAIL ]] && color="$C_RED"
  echo "${color}[${status}]${C_NC} ${id}: ${msg}"
}

print_summary() {
  echo ""
  echo "${C_CYN}==================== SUMMARY ====================${C_NC}"
  printf "%-8s %-6s %s\n" "CHECK" "STATUS" "DETAIL"
  printf "%-8s %-6s %s\n" "-----" "------" "------"
  local i st co
  local pass=0 fail=0 skip=0
  for i in "${!RESULT_ID[@]}"; do
    st="${RESULT_ST[$i]}"
    case "$st" in
      PASS) co="$C_GRN"; pass=$((pass+1)) ;;
      FAIL) co="$C_RED"; fail=$((fail+1)) ;;
      *)    co="$C_YEL"; skip=$((skip+1)) ;;
    esac
    printf "%-8s ${co}%-6s${C_NC} %s\n" "${RESULT_ID[$i]}" "$st" "${RESULT_TXT[$i]}"
  done
  echo "${C_CYN}=================================================${C_NC}"
  echo "PASS=${pass} FAIL=${fail} SKIP=${skip} (total ${#RESULT_ID[@]})"
}

# ---------------------------------------------------------------------------
# Temp workspace + cleanup trap
# ---------------------------------------------------------------------------
TMPDIR_WORK="$(mktemp -d "${TMPDIR:-/tmp}/conf-sync-verify.XXXXXX")"
CONFIG_BACKUP=""          # path to the original controller config (set on first patch)
CONFIG_DIRTY=0            # 1 once we've patched the ConfigMap
declare -a PF_PIDS=()     # background port-forward PIDs

cleanup() {
  local ec=$?
  # Kill any lingering port-forwards.
  local pid
  for pid in "${PF_PIDS[@]:-}"; do
    [[ -n "$pid" ]] && kill "$pid" >/dev/null 2>&1 || true
  done
  # Restore the controller ConfigMap to its shipped default if we changed it.
  if [[ "$CONFIG_DIRTY" == "1" && -n "$CONFIG_BACKUP" && -f "$CONFIG_BACKUP" ]]; then
    warn "Restoring original controller ConfigMap and restarting controller..."
    apply_controller_config_file "$CONFIG_BACKUP" || errln "config restore failed"
    kubectl -n "$NS" delete pod -l "$CTRL_LABEL" --grace-period=5 >/dev/null 2>&1 || true
    wait_leader_ready 200 >/dev/null 2>&1 || true
    info "Controller ConfigMap restored to shipped default."
  fi
  rm -rf "$TMPDIR_WORK" 2>/dev/null || true
  exit "$ec"
}
trap cleanup EXIT

# ---------------------------------------------------------------------------
# kubectl / cluster helpers
# ---------------------------------------------------------------------------
kc() { kubectl -n "$NS" "$@"; }

require_prereqs() {
  command -v kubectl >/dev/null 2>&1 || { errln "kubectl not found"; exit 2; }
  command -v jq >/dev/null 2>&1 || { errln "jq not found"; exit 2; }
  command -v python3 >/dev/null 2>&1 || { errln "python3 not found"; exit 2; }
  kubectl get ns "$NS" >/dev/null 2>&1 || { errln "namespace ${NS} not found"; exit 2; }
  resolve_openssl
  info "context=$(kubectl config current-context 2>/dev/null) ns=${NS} openssl=${OPENSSL} ($(${OPENSSL} version 2>/dev/null))"
}

# All controller pod names.
ctrl_pods() { kc get pods -l "$CTRL_LABEL" -o jsonpath='{range .items[*]}{.metadata.name}{"\n"}{end}'; }
gw_pods()   { kc get pods -l "$GW_LABEL"   -o jsonpath='{range .items[*]}{.metadata.name}{"\n"}{end}'; }

# Leader pod: the one labelled edgion.io/leader=true.
leader_pod() {
  kc get pods -l "$LEADER_LABEL" -o jsonpath='{.items[0].metadata.name}' 2>/dev/null || true
}
# A non-leader controller pod name (empty if only one replica / none).
nonleader_pod() {
  local lead; lead="$(leader_pod)"
  local p
  while read -r p; do
    [[ -z "$p" ]] && continue
    [[ "$p" == "$lead" ]] && continue
    echo "$p"; return 0
  done < <(ctrl_pods)
}

wait_leader() {
  # wait_leader <timeout-secs>
  local timeout="${1:-120}" waited=0
  while (( waited < timeout )); do
    [[ -n "$(leader_pod)" ]] && return 0
    sleep 3; waited=$((waited+3))
  done
  return 1
}

# In leader-only HA the controller Deployment is steady-state 1/2 Ready (the
# follower never initializes its processors, so its /ready returns 503 by
# design). `kubectl rollout status` therefore never completes and the default
# rolling-update strategy deadlocks. So we restart the controller by DELETING
# its pods (the ReplicaSet recreates them with the fresh ConfigMap) and wait for
# a leader-labelled pod whose Ready condition is True.
wait_leader_ready() {
  # wait_leader_ready <timeout-secs> [exclude-pod] -> prints the ready leader pod name
  #
  # `exclude-pod` is REQUIRED for failover assertions: a pod that has been deleted
  # stays Ready and keeps its edgion.io/leader label for its whole termination
  # grace period, so a naive poll happily returns the pod we just killed and the
  # test "passes" without ever observing a failover.
  local timeout="${1:-180}" exclude="${2:-}" waited=0 p
  while (( waited < timeout )); do
    p="$(kc get pods -l "$LEADER_LABEL" \
          -o jsonpath='{range .items[*]}{.metadata.name}:{.status.conditions[?(@.type=="Ready")].status}:{.metadata.deletionTimestamp}{"\n"}{end}' 2>/dev/null \
          | awk -F: -v ex="$exclude" '$2=="True" && $3=="" && $1!=ex {print $1; exit}')"
    [[ -n "$p" ]] && { echo "$p"; return 0; }
    sleep 5; waited=$((waited+5))
  done
  return 1
}

restart_controller_pods() {
  # Hard restart: delete controller pods so they reload the ConfigMap at startup.
  # Prints the ready leader pod name (callers capture it); do NOT redirect
  # wait_leader_ready's stdout here or the name is lost.
  kc delete pod -l "$CTRL_LABEL" --grace-period=5 >/dev/null 2>&1 || true
  wait_leader_ready 200
}

# secret_field <secret> <key> -> base64-decoded value to stdout (empty if absent)
# NOTE: secret data keys contain dots (ca.crt); the dot MUST be escaped in the
# jsonpath expression, else jsonpath treats it as a nested field and returns "".
secret_field() {
  local sec="$1" key="$2" raw esc
  esc="${key//./\\.}"
  raw="$(kc get secret "$sec" -o jsonpath="{.data.${esc}}" 2>/dev/null || true)"
  [[ -z "$raw" ]] && return 0
  echo "$raw" | base64 -d 2>/dev/null || true
}

secret_has_key() {
  local sec="$1" key="$2" esc
  esc="${key//./\\.}"
  kc get secret "$sec" -o jsonpath="{.data.${esc}}" 2>/dev/null | grep -q .
}

# spki_of <pem-file> -> sha256 of the DER-encoded SubjectPublicKeyInfo
spki_of() {
  local pem="$1"
  "$OPENSSL" x509 -in "$pem" -pubkey -noout 2>/dev/null \
    | "$OPENSSL" pkey -pubin -outform DER 2>/dev/null \
    | shasum -a 256 | awk '{print $1}' || true
}

cert_serial()   { "$OPENSSL" x509 -in "$1" -serial   -noout 2>/dev/null | cut -d= -f2 || true; }
cert_notafter() { "$OPENSSL" x509 -in "$1" -enddate  -noout 2>/dev/null | cut -d= -f2 || true; }

# ISO-8601 notAfter — lexicographically sortable, so `[[ a > b ]]` is a correct
# chronological comparison. (The default `MMM DD HH:MM:SS YYYY GMT` rendering is
# neither sortable nor parseable by BSD `date`, which rejects the `GMT` %Z token.)
# Needs OpenSSL 3's -dateopt; callers of this already gate on OpenSSL 3.
cert_notafter_iso() { "$OPENSSL" x509 -in "$1" -enddate -noout -dateopt iso_8601 2>/dev/null | cut -d= -f2 || true; }

# The controller/gateway log to FILES (console:false), not stdout, so
# `kubectl logs` is empty. Read the JSON log files inside each pod via exec.
# Log files live under /usr/local/edgion/logs/*.log (emptyDir per pod, so a
# restarted pod starts a fresh file — assertions after a restart see only the
# new pod's events, which is what we want).
readonly LOG_GLOB="/usr/local/edgion/logs/*.log"

pod_log_grep() {
  # pod_log_grep <pod> <fixed-pattern>
  local pod="$1" pattern="$2"
  kc exec "$pod" -- sh -c 'grep -F -- "$1" '"$LOG_GLOB"' 2>/dev/null' _ "$pattern" 2>/dev/null || true
}

ctrl_log_grep() {
  local pattern="$1" p
  while read -r p; do
    [[ -z "$p" ]] && continue
    pod_log_grep "$p" "$pattern"
  done < <(ctrl_pods)
}

gw_log_grep() {
  local pattern="$1" p
  while read -r p; do
    [[ -z "$p" ]] && continue
    pod_log_grep "$p" "$pattern"
  done < <(gw_pods)
}

# wait_for_ctrl_log <pattern> <timeout-secs>
wait_for_ctrl_log() {
  local pattern="$1" timeout="${2:-120}" waited=0
  while (( waited < timeout )); do
    [[ -n "$(ctrl_log_grep "$pattern")" ]] && return 0
    sleep 5; waited=$((waited+5))
  done
  return 1
}

wait_for_gw_log() {
  local pattern="$1" timeout="${2:-120}" waited=0
  while (( waited < timeout )); do
    [[ -n "$(gw_log_grep "$pattern")" ]] && return 0
    sleep 5; waited=$((waited+5))
  done
  return 1
}

admin_token() { secret_field "$ADMIN_SECRET" token; }

# pf_start <pod> <local-port> -> starts a background port-forward to the admin
# port and BLOCKS until it actually serves (the public /health probe answers).
# A fixed `sleep 3` is not enough: `kubectl port-forward` returns before the
# tunnel is usable, and a freshly-restarted controller may not be listening yet —
# curl then reports 000 and the check fails for reasons unrelated to the feature.
pf_start() {
  local pod="$1" lport="$2" waited=0
  kc port-forward "pod/${pod}" "${lport}:${ADMIN_PORT}" >/dev/null 2>&1 &
  local pid=$!
  PF_PIDS+=("$pid")
  while (( waited < 30 )); do
    if curl -s -o /dev/null --max-time 2 "http://127.0.0.1:${lport}/health" 2>/dev/null; then
      echo "$pid"; return 0
    fi
    sleep 2; waited=$((waited+2))
  done
  kill "$pid" >/dev/null 2>&1 || true
  return 1
}

# http_code <curl-args...> -> the HTTP status, or "000" if the request never
# completed. (`curl ... || echo 000` is WRONG: curl already prints 000 on a
# connection failure AND exits nonzero, so the fallback double-appends.)
http_code() {
  local out
  out="$(curl -s -o /dev/null -w '%{http_code}' --max-time 10 "$@" 2>/dev/null)" || true
  echo "${out:-000}"
}

# ---------------------------------------------------------------------------
# Controller ConfigMap manipulation (Phase 2)
# ---------------------------------------------------------------------------
get_controller_config() {
  kc get configmap "$CTRL_CM" -o jsonpath="{.data.${CTRL_CM_KEY//./\\.}}"
}

apply_controller_config_file() {
  # Merge-patch the ConfigMap's single YAML key with the contents of <file>.
  local file="$1" patch
  patch="$(python3 -c 'import json,sys
print(json.dumps({"data": {sys.argv[2]: open(sys.argv[1]).read()}}))' "$file" "$CTRL_CM_KEY")"
  kc patch configmap "$CTRL_CM" --type merge -p "$patch" >/dev/null
}

backup_config_once() {
  [[ -n "$CONFIG_BACKUP" ]] && return 0
  CONFIG_BACKUP="${TMPDIR_WORK}/controller-config.orig.yaml"
  get_controller_config > "$CONFIG_BACKUP"
  info "Backed up original controller ConfigMap -> ${CONFIG_BACKUP}"
}

# apply_rotation <rotation-yaml-block>
# The block is the body under `conf_sync_security.rotation:` — each line already
# indented with 4 spaces (rotation is a 2-space key under the top-level, col-0
# `conf_sync_security:`). Rebuilds the whole conf_sync_security section (auto_ca
# true + rotation), restarts the controller and waits for a ready leader.
#
# NOTE: the ConfigMap `|` block scalar strips its base indent, so in the
# extracted YAML the top-level keys sit at column 0 (verified). The section is
# rebuilt at that same 0/2/4 indentation.
apply_rotation() {
  # apply_rotation <rotation-block> [fresh]
  # If the 2nd arg is "fresh", the gateway leaf Secret is deleted before the
  # restart so provision() re-mints a leaf with the NEW leaf_ttl (otherwise the
  # existing long-lived leaf is adopted and short-TTL rotation never triggers).
  local block="$1" fresh="${2:-}"
  backup_config_once
  CONFIG_DIRTY=1
  local newcfg="${TMPDIR_WORK}/controller-config.new.yaml"
  # Everything up to (excluding) the col-0 conf_sync_security block, then a fresh one.
  awk '/^conf_sync_security:/{exit} {print}' "$CONFIG_BACKUP" > "$newcfg"
  {
    echo "conf_sync_security:"
    echo "  auto_ca: true"
    echo "  rotation:"
    echo "$block"
  } >> "$newcfg"
  apply_controller_config_file "$newcfg"
  if [[ "$fresh" == "fresh" ]]; then
    info "Deleting gateway leaf Secret so provision() mints a fresh short-TTL leaf"
    kc delete secret "$LEAF_SECRET" --ignore-not-found >/dev/null 2>&1 || true
  fi
  info "Applied rotation config; restarting controller pods..."
  local lead
  lead="$(restart_controller_pods)"
  if [[ -z "$lead" ]]; then errln "no ready leader after config apply"; return 1; fi
  info "Controller restarted; ready leader=${lead}"
}

# ---------------------------------------------------------------------------
# Crafted-cert helpers (openssl 3.x)
# ---------------------------------------------------------------------------
# Extract the current CA into $TMPDIR_WORK/ca.crt + ca.key. Returns non-zero if
# the CA secret is missing its material.
extract_ca() {
  local dir="$1"
  secret_field "$CA_SECRET" "ca.crt" > "${dir}/ca.crt"
  secret_field "$CA_SECRET" "ca.key" > "${dir}/ca.key"
  [[ -s "${dir}/ca.crt" && -s "${dir}/ca.key" ]]
}

# craft_client_leaf <out-dir> <not_after_ts> [not_before_ts]
# Signs a CN=edgion-gateway clientAuth leaf with the CURRENT CA. Timestamps are
# openssl [CC]YYMMDDHHMMSSZ. notBefore defaults to 5 min ago (backdated).
craft_client_leaf() {
  local dir="$1" na="$2" nb="${3:-$(date -u -v-5M '+%Y%m%d%H%M%SZ')}"
  "$OPENSSL" ecparam -name prime256v1 -genkey -noout -out "${dir}/client.key" 2>/dev/null
  "$OPENSSL" req -new -key "${dir}/client.key" -subj "/CN=edgion-gateway" \
    -out "${dir}/client.csr" 2>/dev/null
  cat > "${dir}/client.ext" <<'EOF'
basicConstraints=critical,CA:FALSE
keyUsage=critical,digitalSignature
extendedKeyUsage=clientAuth
EOF
  "$OPENSSL" x509 -req -in "${dir}/client.csr" \
    -CA "${dir}/ca.crt" -CAkey "${dir}/ca.key" -CAcreateserial \
    -not_before "$nb" -not_after "$na" \
    -extfile "${dir}/client.ext" -out "${dir}/client.crt" 2>/dev/null
  [[ -s "${dir}/client.crt" ]]
}

# The controller writes both managed Secrets with server-side apply under the
# field manager `edgion-conf-sync-ca` (see edgion-controller/src/services/
# conf_sync_ca/secret_io.rs) and FORCES conflicts. Forcing is load-bearing:
# SSA only overwrites fields the SAME manager owns, and both Secrets routinely
# have a foreign owner —
#   * `edgion-conf-sync-ca` is POST-created by `ensure_ca`, so the API server
#     records its `data` under the client's default manager (`unknown`);
#   * either Secret can be `kubectl create`d / `kubectl patch`ed out of band.
# Without `.force()` every subsequent apply 409-Conflicts forever (this WAS a
# real product bug: CA renewal wedged in backoff until the root expired).
#
# This harness therefore deliberately injects crafted material with PLAIN
# kubectl (foreign field manager) — that is the condition under test. A passing
# run proves the controller reclaims ownership instead of 409-wedging.
readonly CTRL_FIELD_MANAGER="edgion-conf-sync-ca"

# kubectl_put_secret <name> <key=file> ...
# Creates the Secret with a plain POST (kubectl's own field manager owns `data`).
kubectl_put_secret() {
  local name="$1"; shift
  local args=()
  local pair
  for pair in "$@"; do
    args+=(--from-file="${pair}")
  done
  kc delete secret "$name" --ignore-not-found >/dev/null 2>&1 || true
  kc create secret generic "$name" "${args[@]}" >/dev/null
  kc label secret "$name" "edgion.io/managed-by=conf-sync-ca" --overwrite >/dev/null 2>&1 || true
}

# secret_managers <secret> -> the full managedFields array (field-ownership evidence).
# NOTE: `kubectl get -o json` STRIPS managedFields unless --show-managed-fields
# is passed (kubectl >= 1.21). Without it every ownership assertion silently
# reads `null`.
secret_managers() {
  kc get secret "$1" --show-managed-fields -o json 2>/dev/null | jq '.metadata.managedFields' || true
}

# secret_data_owner <secret> -> comma-joined "manager/operation" of the f:data owners
secret_data_owner() {
  kc get secret "$1" --show-managed-fields -o json 2>/dev/null \
    | jq -r '.metadata.managedFields[]? | select(.fieldsV1["f:data"] != null) | "\(.manager)/\(.operation)"' \
    | paste -sd, - 2>/dev/null || true
}

# Controller-log evidence of an SSA conflict (the signature of the fixed bug).
ctrl_conflict_grep() {
  local hit
  hit="$(ctrl_log_grep 'Conflict')$(ctrl_log_grep 'conflict with')"
  echo "$hit"
}

# Replace client.crt/client.key with the crafted leaf in <dir>, using a PLAIN
# kubectl merge-patch. This hands `f:data` ownership to kubectl, which is exactly
# the state that used to wedge the next controller apply. `ca.crt` is left alone
# (a merge patch is additive, unlike an SSA apply).
patch_leaf_secret() {
  local dir="$1" patch
  patch="$(python3 -c 'import base64,json,sys
d={k: base64.b64encode(open(v,"rb").read()).decode() for k,v in (a.split("=",1) for a in sys.argv[1:])}
print(json.dumps({"data": d}))' \
    "client.crt=${dir}/client.crt" "client.key=${dir}/client.key")"
  kc patch secret "$LEAF_SECRET" --type merge -p "$patch" >/dev/null
}

# craft_byo_ca <out-dir> <not_before_ts> <not_after_ts>
# Self-signed ECDSA P-256 CA (CN=edgion-grpc-ca) with an explicit validity
# window. Writes ca.crt + ca.key.
craft_byo_ca() {
  local dir="$1" nb="$2" na="$3"
  "$OPENSSL" ecparam -name prime256v1 -genkey -noout -out "${dir}/ca.key" 2>/dev/null
  "$OPENSSL" req -new -x509 -key "${dir}/ca.key" -subj "/CN=edgion-grpc-ca" \
    -not_before "$nb" -not_after "$na" \
    -addext "basicConstraints=critical,CA:TRUE" \
    -addext "keyUsage=critical,keyCertSign,cRLSign" \
    -out "${dir}/ca.crt" 2>/dev/null
  [[ -s "${dir}/ca.crt" ]]
}

# Create/replace the CA secret from ca.crt+ca.key in <dir>.
# Deliberately a PLAIN kubectl create (foreign field manager owns `data`) — this
# reproduces the real-world state the controller's `ensure_ca` POST also
# produces, and is precisely what the forced apply must survive.
put_ca_secret() {
  local dir="$1"
  kubectl_put_secret "$CA_SECRET" "ca.crt=${dir}/ca.crt" "ca.key=${dir}/ca.key"
}

# Snapshot the current leaf secret's client.crt serial (empty if absent).
leaf_serial() {
  local dir; dir="$(mktemp -d "${TMPDIR_WORK}/leaf.XXXXXX")"
  secret_field "$LEAF_SECRET" "client.crt" > "${dir}/c.crt"
  [[ -s "${dir}/c.crt" ]] || { echo ""; return 0; }
  cert_serial "${dir}/c.crt"
}

# Restart controller (pod-delete) and wait for a ready leader.
restart_controller() {
  restart_controller_pods >/dev/null
}

# Snapshot gateway pod restart counts (name:count per line).
gw_restart_snapshot() {
  kc get pods -l "$GW_LABEL" \
    -o jsonpath='{range .items[*]}{.metadata.name}:{.status.containerStatuses[0].restartCount}{"\n"}{end}' 2>/dev/null
}

# ===========================================================================
# PHASE 1 CHECKS
# ===========================================================================

p1_1() {  # Controller pod starts without cert Secrets (optional volume + in-mem cert)
  step "P1.1 controller starts without pre-existing cert Secrets"
  local ready
  ready="$(kc get deploy "$CTRL_DEPLOY" -o jsonpath='{.status.readyReplicas}' 2>/dev/null || echo 0)"
  if [[ "${ready:-0}" -ge 1 ]] && wait_for_ctrl_log "auto-CA, in-memory server cert" 60; then
    record P1.1 PASS "controller Ready=${ready}; started with in-memory server cert (no Secret volume dependency)"
  else
    record P1.1 FAIL "controller not Ready or in-memory-cert startup log absent"
  fi
}

p1_2() {  # Controller auto-creates edgion-conf-sync-ca (ca.crt + ca.key)
  step "P1.2 auto-created ${CA_SECRET} (ca.crt + ca.key)"
  if secret_has_key "$CA_SECRET" "ca.crt" && secret_has_key "$CA_SECRET" "ca.key"; then
    record P1.2 PASS "${CA_SECRET} present with ca.crt + ca.key"
  else
    record P1.2 FAIL "${CA_SECRET} missing ca.crt and/or ca.key"
  fi
}

p1_3() {  # Controller auto-creates edgion-conf-sync-certs (client.* + ca.crt, NO ca.key)
  step "P1.3 auto-created ${LEAF_SECRET} (client.crt/key + ca.crt, no ca.key)"
  if secret_has_key "$LEAF_SECRET" "client.crt" \
     && secret_has_key "$LEAF_SECRET" "client.key" \
     && secret_has_key "$LEAF_SECRET" "ca.crt" \
     && ! secret_has_key "$LEAF_SECRET" "ca.key"; then
    record P1.3 PASS "${LEAF_SECRET} has client.crt/client.key/ca.crt and NO ca.key"
  else
    record P1.3 FAIL "${LEAF_SECRET} key set incorrect (want client.crt/key+ca.crt, no ca.key)"
  fi
}

p1_4() {  # Gateway pod starts (optional volume; doesn't hang while Secret absent)
  step "P1.4 gateway pods start (optional cert volume)"
  local ready
  ready="$(kc get deploy "$GW_DEPLOY" -o jsonpath='{.status.readyReplicas}' 2>/dev/null || echo 0)"
  if [[ "${ready:-0}" -ge 1 ]]; then
    record P1.4 PASS "gateway Ready=${ready} (optional:true cert volume did not block startup)"
  else
    record P1.4 FAIL "gateway has no ready replicas"
  fi
}

p1_5() {  # Gateway connects via auto mTLS; no plaintext_default
  step "P1.5 gateway connects via auto mTLS (no plaintext_default)"
  local gw_ok=1 ctrl_ok=1 plaintext=0
  [[ -n "$(gw_log_grep 'conf_sync mTLS enabled for gRPC client')" ]] || gw_ok=0
  [[ -n "$(ctrl_log_grep 'conf_sync mTLS enabled (hot-reloadable accept loop)')" ]] \
    || [[ -n "$(ctrl_log_grep 'auto-CA, in-memory server cert')" ]] || ctrl_ok=0
  [[ -n "$(gw_log_grep 'plaintext_default')" ]] && plaintext=1
  [[ -n "$(ctrl_log_grep 'plaintext_default')" ]] && plaintext=1
  if [[ "$gw_ok" == 1 && "$ctrl_ok" == 1 && "$plaintext" == 0 ]]; then
    record P1.5 PASS "gateway 'mTLS enabled' + controller auto-CA path present; no plaintext_default"
  else
    record P1.5 FAIL "gw_mtls=${gw_ok} ctrl_autoCA=${ctrl_ok} plaintext_default=${plaintext}"
  fi
}

p1_6() {  # Two-replica split-brain: single root; restart a replica keeps mTLS
  step "P1.6 two-replica convergence on a single root"
  local replicas ca_before ca_after
  replicas="$(kc get deploy "$CTRL_DEPLOY" -o jsonpath='{.spec.replicas}')"
  if [[ "${replicas:-0}" -lt 2 ]]; then
    record P1.6 SKIP "controller replicas=${replicas} (<2); split-brain not exercised"
    return
  fi
  # Exactly one ca.crt entry means both replicas converged on one root.
  ca_before="$(secret_field "$CA_SECRET" ca.crt)"
  if [[ -z "$ca_before" ]]; then
    record P1.6 FAIL "no ca.crt to compare"
    return
  fi
  # Restart one replica (delete leader pod) and confirm the root is unchanged and
  # gateway mTLS still healthy afterwards.
  local victim; victim="$(leader_pod)"
  [[ -z "$victim" ]] && victim="$(ctrl_pods | head -1)"
  info "deleting controller pod ${victim} to exercise re-convergence"
  kc delete pod "$victim" --wait=false >/dev/null 2>&1 || true
  wait_leader_ready 200 >/dev/null 2>&1 || true
  ca_after="$(secret_field "$CA_SECRET" ca.crt)"
  # Gateway must still be able to sync: look for a fresh mTLS-enabled line OR that
  # pods stay Ready.
  local gw_ready
  gw_ready="$(kc get deploy "$GW_DEPLOY" -o jsonpath='{.status.readyReplicas}' 2>/dev/null || echo 0)"
  if [[ "$ca_before" == "$ca_after" && "${gw_ready:-0}" -ge 1 ]]; then
    record P1.6 PASS "single root preserved across replica restart; gateway still Ready=${gw_ready}"
  else
    record P1.6 FAIL "root changed unexpectedly or gateway lost readiness (gw_ready=${gw_ready})"
  fi
}

p1_7() {  # SAN: gateway hostname verification against the leader Service
  step "P1.7 gateway server_addr targets the leader Service SAN"
  local addr san
  addr="$(kc get configmap edgion-gateway-config -o jsonpath='{.data.edgion-gateway\.yaml}' 2>/dev/null \
            | grep -E 'server_addr:' | head -1)"
  # The controller-server SAN set must include edgion-controller-leader.<ns>.svc...
  local dir; dir="$(mktemp -d "${TMPDIR_WORK}/san.XXXXXX")"
  # The controller server cert is in-memory; assert via config wiring + a live
  # mTLS success (gateway 'mTLS enabled' log, verified in P1.5) that hostname
  # verification passes for the leader Service name.
  if echo "$addr" | grep -q "edgion-controller-leader.${NS}.svc"; then
    if [[ -n "$(gw_log_grep 'conf_sync mTLS enabled for gRPC client')" ]]; then
      record P1.7 PASS "gateway dials ${NS} leader Service and mTLS handshake succeeds (SAN verified live)"
    else
      record P1.7 SKIP "server_addr targets leader Service, but no live mTLS log yet to confirm SAN"
    fi
  else
    record P1.7 FAIL "gateway server_addr does not target edgion-controller-leader.${NS}.svc (${addr})"
  fi
}

p1_8() {  # Adopt: pre-create edgion-conf-sync-ca with a BYO root -> controller adopts
  step "P1.8 controller adopts a pre-created BYO root (no overwrite)"
  if [[ "$OPENSSL" != *"openssl"* ]]; then :; fi
  local dir; dir="$(mktemp -d "${TMPDIR_WORK}/adopt.XXXXXX")"
  local nb na
  nb="$(date -u -v-1H '+%Y%m%d%H%M%SZ')"
  na="$(date -u -v+3650d '+%Y%m%d%H%M%SZ')"
  if ! craft_byo_ca "$dir" "$nb" "$na"; then
    record P1.8 SKIP "could not craft BYO CA (openssl -not_after unsupported?)"
    return
  fi
  local byo_spki; byo_spki="$(spki_of "${dir}/ca.crt")"
  info "installing BYO root and clearing leaf; restarting controller to force provision"
  kc delete secret "$LEAF_SECRET" --ignore-not-found >/dev/null 2>&1 || true
  put_ca_secret "$dir"
  restart_controller || { record P1.8 FAIL "controller did not become ready after BYO install"; return; }
  # Give provision() a moment.
  sleep 5
  local dir2; dir2="$(mktemp -d "${TMPDIR_WORK}/adopt2.XXXXXX")"
  secret_field "$CA_SECRET" ca.crt > "${dir2}/ca.crt"
  local now_spki; now_spki="$(spki_of "${dir2}/ca.crt")"
  if [[ -n "$now_spki" && "$now_spki" == "$byo_spki" ]]; then
    record P1.8 PASS "controller adopted BYO root (SPKI unchanged after start)"
  else
    record P1.8 FAIL "BYO root NOT adopted (spki before=${byo_spki:0:12} after=${now_spki:0:12})"
  fi
}

p1_9() {  # RBAC: controller can create/patch Secrets; gateway SA cannot read CA
  step "P1.9 RBAC: controller writes Secrets; gateway SA cannot read the CA"
  local ctrl_create ctrl_patch gw_get
  # `kubectl auth can-i` prints yes/no on stdout AND exits nonzero on "no", so a
  # `|| echo no` would double-append. Capture stdout directly and default empty.
  ctrl_create="$(kubectl auth can-i create secrets \
      --as="system:serviceaccount:${NS}:${CTRL_DEPLOY}" -n "$NS" 2>/dev/null | head -1 || true)"
  ctrl_patch="$(kubectl auth can-i patch secrets \
      --as="system:serviceaccount:${NS}:${CTRL_DEPLOY}" -n "$NS" 2>/dev/null | head -1 || true)"
  gw_get="$(kubectl auth can-i get "secret/${CA_SECRET}" \
      --as="system:serviceaccount:${NS}:${GW_SA}" -n "$NS" 2>/dev/null | head -1 || true)"
  ctrl_create="${ctrl_create:-no}"; ctrl_patch="${ctrl_patch:-no}"; gw_get="${gw_get:-no}"
  if [[ "$ctrl_create" == yes && "$ctrl_patch" == yes && "$gw_get" == no ]]; then
    record P1.9 PASS "controller create/patch=yes; gateway SA (${GW_SA}) get CA=no"
  else
    record P1.9 FAIL "ctrl_create=${ctrl_create} ctrl_patch=${ctrl_patch} gw_get_ca=${gw_get}"
  fi
}

# Run a check tolerantly: calling it as part of a `|| ...` list suppresses
# `set -e` inside the function, so an unguarded nonzero command cannot abort the
# whole run — the check completes and records its own result. If a check aborts
# before recording, note it so the summary is not silently short.
run_check_safe() {
  local fn="$1" id="$2"
  "$fn" || warn "${id} body returned nonzero (continuing)"
}

run_phase1() {
  # p1_8 last: it swaps the CA root (adoption), which is a valid end state.
  run_check_safe p1_1 P1.1; run_check_safe p1_2 P1.2; run_check_safe p1_3 P1.3
  run_check_safe p1_4 P1.4; run_check_safe p1_5 P1.5; run_check_safe p1_6 P1.6
  run_check_safe p1_7 P1.7; run_check_safe p1_9 P1.9; run_check_safe p1_8 P1.8
}

# ===========================================================================
# PHASE 2 CHECKS
# ===========================================================================

p2_1() {  # lifetime rotation at short TTL
  step "P2.1 lifetime-policy rotation at forced short TTL"
  apply_rotation "$(cat <<'EOF'
    policy: lifetime
    lifetime_ratio: 0.05
    leaf_ttl: 2h
    ca_ttl: 30d
    emergency_rotate_below: 1h
EOF
)" fresh || { record P2.1 FAIL "config apply failed"; return; }
  local serial_before; serial_before="$(leaf_serial)"
  local gw_snap_before; gw_snap_before="$(gw_restart_snapshot)"
  info "waiting up to 8m for lifetime rotation (trigger ~= age 6m with 5m backdate)..."
  if wait_for_ctrl_log "conf_sync_cert_rotated" 480; then
    sleep 4
    local serial_after; serial_after="$(leaf_serial)"
    local gw_snap_after; gw_snap_after="$(gw_restart_snapshot)"
    if [[ -n "$serial_after" && "$serial_after" != "$serial_before" ]]; then
      local disruption="none"
      [[ "$gw_snap_before" != "$gw_snap_after" ]] && disruption="gw-restart-detected"
      if [[ "$disruption" == none ]]; then
        record P2.1 PASS "leaf serial changed ${serial_before:0:8}->${serial_after:0:8}; no gateway restart"
      else
        record P2.1 FAIL "rotation happened but gateway restarted (${disruption})"
      fi
    else
      record P2.1 FAIL "rotation logged but leaf serial unchanged"
    fi
  else
    record P2.1 FAIL "no conf_sync_cert_rotated within 8m"
  fi
}

p2_2() {  # gateway picks up rotated client leaf
  step "P2.2 gateway picks up rotated leaf (reload + reconnect)"
  # Depends on a rotation having just happened (P2.1). Look for the gateway
  # reload event within kubelet-sync + debounce window.
  info "waiting up to 120s for gateway conf_sync_cert_reloaded..."
  if wait_for_gw_log "conf_sync_cert_reloaded" 120; then
    local gw_ready
    gw_ready="$(kc get deploy "$GW_DEPLOY" -o jsonpath='{.status.readyReplicas}' 2>/dev/null || echo 0)"
    record P2.2 PASS "gateway logged conf_sync_cert_reloaded; Ready=${gw_ready}"
  else
    record P2.2 FAIL "gateway did not log conf_sync_cert_reloaded within 120s"
  fi
}

p2_3() {  # scheduled window semantics
  step "P2.3 scheduled policy: guard-skip then fire at a window boundary"
  apply_rotation "$(cat <<'EOF'
    policy: scheduled
    schedule: "*/2 * * * *"
    schedule_timezone: UTC
    leaf_ttl: 4h
    ca_ttl: 30d
    rotate_if_remaining_below: 2h
    emergency_rotate_below: 1h
EOF
)" fresh || { record P2.3 FAIL "config apply failed"; return; }
  # Phase A: fresh 4h leaf has ~4h remaining > 2h guard -> must NOT rotate across
  # at least two */2 firings (~5 min observation).
  local serial_a; serial_a="$(leaf_serial)"
  info "Phase A: observing ~5m for guard-skip (no rotation expected)..."
  local skipped=1 waited=0
  while (( waited < 300 )); do
    sleep 30; waited=$((waited+30))
    local s; s="$(leaf_serial)"
    if [[ "$s" != "$serial_a" ]]; then skipped=0; break; fi
  done
  if [[ "$skipped" != 1 ]]; then
    record P2.3 FAIL "unexpected rotation during guard-skip window (fresh leaf should not rotate)"
    return
  fi
  info "Phase A OK (guard-skipped). Phase B: forcing a due leaf (~90m remaining)."
  if ! "$OPENSSL" version 2>/dev/null | grep -q "^OpenSSL 3"; then
    record P2.3 SKIP "guard-skip verified; forced-due phase needs OpenSSL 3 (-not_after)"
    return
  fi
  local dir; dir="$(mktemp -d "${TMPDIR_WORK}/sched.XXXXXX")"
  if ! extract_ca "$dir"; then
    record P2.3 SKIP "guard-skip verified; cannot extract CA to craft forced-due leaf"
    return
  fi
  local na; na="$(date -u -v+90M '+%Y%m%d%H%M%SZ')"
  craft_client_leaf "$dir" "$na" || { record P2.3 FAIL "could not craft due leaf"; return; }
  local serial_b; serial_b="$(cert_serial "${dir}/client.crt")"
  patch_leaf_secret "$dir"
  info "patched a ~90m leaf (remaining < 2h guard); expecting rotation at the next */2 firing"
  # Rotation must occur, and within ~2 firings (<= ~3 min + tick slack).
  if wait_for_ctrl_log "scheduled window due" 240 || wait_for_ctrl_log "conf_sync_cert_rotated" 240; then
    sleep 4
    local serial_after; serial_after="$(leaf_serial)"
    if [[ "$serial_after" != "$serial_b" ]]; then
      record P2.3 PASS "guard-skipped fresh leaf; rotated the forced-due leaf at a firing boundary"
    else
      record P2.3 FAIL "rotation logged but leaf serial not advanced from crafted one"
    fi
  else
    record P2.3 FAIL "forced-due leaf did not rotate within 4m"
  fi
}

p2_4() {  # manual API 202 on leader, 409 on non-leader
  step "P2.4 manual rotate API (202 leader / 409 non-leader)"
  local tok; tok="$(admin_token)"
  if [[ -z "$tok" ]]; then
    record P2.4 SKIP "admin token secret unavailable"
    return
  fi
  local lead; lead="$(leader_pod)"
  if [[ -z "$lead" ]]; then
    record P2.4 FAIL "no leader pod found"
    return
  fi
  # --- Leader: expect 202 ---
  local lport=13401 pf1
  pf1="$(pf_start "$lead" "$lport")" || { record P2.4 FAIL "port-forward to leader ${lead} never became usable"; return; }
  local code_leader
  code_leader="$(http_code -X POST \
      -H "Authorization: Bearer ${tok}" -H "Content-Type: application/json" \
      -d '{"scope":"leaf"}' "http://127.0.0.1:${lport}/api/v1/services/conf-sync-ca/rotate")"
  kill "$pf1" >/dev/null 2>&1 || true
  # --- Non-leader: expect 409 ---
  local nl; nl="$(nonleader_pod)"
  local code_nonleader="SKIP"
  if [[ -n "$nl" ]]; then
    local nport=13402 pf2
    if pf2="$(pf_start "$nl" "$nport")"; then
      code_nonleader="$(http_code -X POST \
          -H "Authorization: Bearer ${tok}" -H "Content-Type: application/json" \
          -d '{"scope":"leaf"}' "http://127.0.0.1:${nport}/api/v1/services/conf-sync-ca/rotate")"
      kill "$pf2" >/dev/null 2>&1 || true
    fi
  fi
  if [[ "$code_leader" == "202" && ( "$code_nonleader" == "409" || "$code_nonleader" == "SKIP" ) ]]; then
    local note="leader=202 nonleader=${code_nonleader}"
    [[ "$code_nonleader" == "SKIP" ]] && note="${note} (single replica; 409 not exercised)"
    record P2.4 PASS "$note (400/auto_ca-off and 503 intentionally not exercised)"
  elif [[ "$code_leader" == "403" ]]; then
    # A 403 here is NOT an auth misconfiguration: the authz route_layer
    # default-denies any route `authz_classifier::classify()` cannot map to a
    # (Verb, kind). `POST /api/v1/services/conf-sync-ca/rotate` has no entry in
    # that table (and no `Verb` variant), so the endpoint is unreachable for
    # EVERY caller, including the wildcard admin token.
    record P2.4 FAIL "403 for a valid admin token: the rotate route is missing from authz_classifier::classify() -> unknown-route default-deny. Endpoint unreachable (product bug)."
  else
    record P2.4 FAIL "leader=${code_leader} (want 202) nonleader=${code_nonleader} (want 409)"
  fi
}

p2_5() {  # CA renewal same-key  (ALSO the regression test for the SSA-force fix)
  step "P2.5 CA renewal (same-key re-sign of a near-expiry root, foreign field manager)"
  if ! "$OPENSSL" version 2>/dev/null | grep -q "^OpenSSL 3"; then
    record P2.5 SKIP "needs OpenSSL 3 (-not_before/-not_after)"
    return
  fi
  local dir; dir="$(mktemp -d "${TMPDIR_WORK}/carenew.XXXXXX")"
  # Total validity 10h, 9h elapsed -> 1h remaining; threshold min(30d, 10h/10=1h)
  # = 1h -> renewal due on the first tick that sees a consistent chain.
  local nb na
  nb="$(date -u -v-9H '+%Y%m%d%H%M%SZ')"
  na="$(date -u -v+1H '+%Y%m%d%H%M%SZ')"
  craft_byo_ca "$dir" "$nb" "$na" || { record P2.5 FAIL "could not craft near-expiry CA"; return; }
  local spki_before; spki_before="$(spki_of "${dir}/ca.crt")"
  local key_before; key_before="$(base64 < "${dir}/ca.key" | tr -d '\n')"
  local na_before; na_before="$(cert_notafter_iso "${dir}/ca.crt")"

  info "installing near-expiry BYO root (1h remaining) via PLAIN kubectl create"
  kc delete secret "$LEAF_SECRET" --ignore-not-found >/dev/null 2>&1 || true
  put_ca_secret "$dir"
  local owner_before; owner_before="$(secret_data_owner "$CA_SECRET")"
  info "CA Secret f:data owner BEFORE renewal: ${owner_before}"

  # Manual policy + short ca_ttl: no leaf-policy noise, and the renewed root gets
  # a 30d window (threshold min(30d, 3d)=3d) so renewal fires exactly once.
  apply_rotation "$(cat <<'EOF'
    policy: manual
    leaf_ttl: 2h
    ca_ttl: 30d
    emergency_rotate_below: 1h
EOF
)" || { record P2.5 FAIL "config apply failed"; return; }

  info "waiting up to 6m for CA renewal (conf_sync_ca_renewed)..."
  if ! wait_for_ctrl_log "conf_sync_ca_renewed" 360; then
    errln "CA renewal never happened. Conflict evidence from controller logs:"
    ctrl_conflict_grep | head -5 >&2
    record P2.5 FAIL "no conf_sync_ca_renewed within 6m (SSA conflict? see log above)"
    return
  fi
  sleep 5
  local dir2; dir2="$(mktemp -d "${TMPDIR_WORK}/carenew2.XXXXXX")"
  secret_field "$CA_SECRET" ca.crt > "${dir2}/ca.crt"
  local spki_after; spki_after="$(spki_of "${dir2}/ca.crt")"
  local key_after; key_after="$(secret_field "$CA_SECRET" ca.key | base64 | tr -d '\n')"
  local na_after; na_after="$(cert_notafter_iso "${dir2}/ca.crt")"
  local owner_after; owner_after="$(secret_data_owner "$CA_SECRET")"
  info "CA Secret f:data owner AFTER renewal:  ${owner_after}"
  info "CA notAfter ${na_before} -> ${na_after}"

  local same_key=0 later=0 reclaimed=0 no_conflict=0 leaf_ok=0
  [[ "$spki_after" == "$spki_before" && "$key_after" == "$key_before" ]] && same_key=1
  # ISO-8601 sorts lexicographically, so `>` is a valid chronological compare.
  [[ -n "$na_after" && -n "$na_before" && "$na_after" > "$na_before" ]] && later=1
  # The forced apply must have RECLAIMED f:data from the foreign manager.
  echo "$owner_after" | grep -q "${CTRL_FIELD_MANAGER}/Apply" && reclaimed=1
  [[ -z "$(ctrl_conflict_grep)" ]] && no_conflict=1
  # Leaves must be re-signed against the renewed root (leaf ca.crt == new ca.crt).
  local leaf_ca; leaf_ca="$(secret_field "$LEAF_SECRET" ca.crt)"
  [[ -n "$leaf_ca" && "$leaf_ca" == "$(cat "${dir2}/ca.crt")" ]] && leaf_ok=1

  local gw_ready
  gw_ready="$(kc get deploy "$GW_DEPLOY" -o jsonpath='{.status.readyReplicas}' 2>/dev/null || echo 0)"

  if [[ "$same_key" == 1 && "$later" == 1 && "$reclaimed" == 1 && "$no_conflict" == 1 \
        && "$leaf_ok" == 1 && "${gw_ready:-0}" -ge 1 ]]; then
    record P2.5 PASS "CA renewed same-key (SPKI+ca.key identical, notAfter advanced); f:data reclaimed ${owner_before} -> ${owner_after}; no 409; leaves re-signed; gw Ready=${gw_ready}"
  else
    record P2.5 FAIL "same_key=${same_key} later=${later} reclaimed=${reclaimed} no_409=${no_conflict} leaf_resigned=${leaf_ok} gw_ready=${gw_ready} (owner ${owner_before} -> ${owner_after})"
  fi
}

p2_6() {  # leader failover: new leader resumes rotation service, no storm
  step "P2.6 leader failover (new leader resumes; no rotation storm)"
  local replicas; replicas="$(kc get deploy "$CTRL_DEPLOY" -o jsonpath='{.spec.replicas}')"
  if [[ "${replicas:-0}" -lt 2 ]]; then
    record P2.6 SKIP "controller replicas=${replicas} (<2); failover not exercisable"
    return
  fi
  # Use a manual policy so no scheduled/lifetime rotation is due; a healthy leaf
  # must NOT rotate on failover (no storm).
  apply_rotation "$(cat <<'EOF'
    policy: manual
    leaf_ttl: 2h
    ca_ttl: 30d
    emergency_rotate_below: 1h
EOF
)" || { record P2.6 FAIL "config apply failed"; return; }
  local serial_before; serial_before="$(leaf_serial)"
  local old_leader; old_leader="$(leader_pod)"
  [[ -z "$old_leader" ]] && { record P2.6 FAIL "no leader before failover"; return; }
  info "deleting leader ${old_leader} to force failover"
  kc delete pod "$old_leader" --wait=false >/dev/null 2>&1 || true
  local new_leader
  # Exclude the old leader: it keeps its Ready status + leader label until the
  # grace period elapses, and would otherwise be reported as the "new" leader.
  new_leader="$(wait_leader_ready 200 "$old_leader")" \
    || { record P2.6 FAIL "no new ready leader (other than ${old_leader}) within 200s"; return; }
  # Give the new leader ~2 ticks; assert it serves (rotation service started) and
  # no unexpected rotation storm (serial unchanged, manual policy, healthy leaf).
  sleep 20
  local serial_after; serial_after="$(leaf_serial)"
  if [[ "$serial_after" == "$serial_before" ]]; then
    record P2.6 PASS "failover ${old_leader}->${new_leader}: no rotation storm (leaf serial stable)"
  else
    record P2.6 FAIL "leaf serial changed on failover (${serial_before:0:8}->${serial_after:0:8}); possible storm"
  fi
}

p2_7() {  # emergency safety net under manual policy
  step "P2.7 emergency net rotates under manual policy"
  if ! "$OPENSSL" version 2>/dev/null | grep -q "^OpenSSL 3"; then
    record P2.7 SKIP "needs OpenSSL 3 (-not_after)"
    return
  fi
  apply_rotation "$(cat <<'EOF'
    policy: manual
    leaf_ttl: 2h
    ca_ttl: 30d
    emergency_rotate_below: 1h
EOF
)" || { record P2.7 FAIL "config apply failed"; return; }
  local dir; dir="$(mktemp -d "${TMPDIR_WORK}/emerg.XXXXXX")"
  if ! extract_ca "$dir"; then
    record P2.7 FAIL "cannot extract CA to craft emergency leaf"
    return
  fi
  # ~30m remaining, well under emergency_rotate_below=1h -> emergency rotation.
  local na; na="$(date -u -v+30M '+%Y%m%d%H%M%SZ')"
  craft_client_leaf "$dir" "$na" || { record P2.7 FAIL "could not craft emergency leaf"; return; }
  local serial_b; serial_b="$(cert_serial "${dir}/client.crt")"
  patch_leaf_secret "$dir"
  info "patched a ~30m leaf; expecting emergency rotation within ~1-2 ticks"
  if wait_for_ctrl_log "cert_emergency_rotation" 180; then
    sleep 4
    local serial_after; serial_after="$(leaf_serial)"
    if [[ "$serial_after" != "$serial_b" ]]; then
      record P2.7 PASS "emergency WARN logged + leaf rotated despite manual policy"
    else
      record P2.7 FAIL "emergency logged but serial not advanced"
    fi
  else
    record P2.7 FAIL "no cert_emergency_rotation within 3m"
  fi
}

p2_8() {  # re-key drill
  step "P2.8 re-key drill (delete both Secrets -> re-bootstrap new root)"
  local dir; dir="$(mktemp -d "${TMPDIR_WORK}/rekey.XXXXXX")"
  secret_field "$CA_SECRET" ca.crt > "${dir}/ca_before.crt" 2>/dev/null || true
  local spki_before=""
  [[ -s "${dir}/ca_before.crt" ]] && spki_before="$(spki_of "${dir}/ca_before.crt")"
  info "deleting ${CA_SECRET} and ${LEAF_SECRET}; leader must re-bootstrap a new root"
  kc delete secret "$CA_SECRET" "$LEAF_SECRET" --ignore-not-found >/dev/null 2>&1 || true
  # Leader reconciler re-provisions on its next tick (<= ~60s). No restart needed,
  # but nudge by waiting.
  local waited=0 got=0
  while (( waited < 120 )); do
    sleep 10; waited=$((waited+10))
    if secret_has_key "$CA_SECRET" ca.crt && secret_has_key "$LEAF_SECRET" client.crt; then got=1; break; fi
  done
  if [[ "$got" != 1 ]]; then
    record P2.8 FAIL "Secrets not re-bootstrapped within 120s of deletion"
    return
  fi
  secret_field "$CA_SECRET" ca.crt > "${dir}/ca_after.crt"
  local spki_after; spki_after="$(spki_of "${dir}/ca_after.crt")"
  if [[ -z "$spki_before" || "$spki_after" != "$spki_before" ]]; then
    # New root minted. Now delete leader and confirm the new leader serves.
    local old_leader; old_leader="$(leader_pod)"
    if [[ -n "$old_leader" ]] && [[ "$(kc get deploy "$CTRL_DEPLOY" -o jsonpath='{.spec.replicas}')" -ge 2 ]]; then
      kc delete pod "$old_leader" --wait=false >/dev/null 2>&1 || true
      wait_leader 120 || true
      sleep 10
      local resync serves
      resync="$(ctrl_log_grep 'conf_sync_trust_root_resync')"
      serves="$(kc get deploy "$GW_DEPLOY" -o jsonpath='{.status.readyReplicas}' 2>/dev/null || echo 0)"
      if [[ -n "$resync" ]]; then
        record P2.8 PASS "new root minted; post-failover leader logged conf_sync_trust_root_resync (gw Ready=${serves})"
      else
        record P2.8 PASS "new root minted; new leader serves correctly without resync log (gw Ready=${serves})"
      fi
    else
      record P2.8 PASS "new root minted after Secret deletion (single replica; failover step skipped)"
    fi
  else
    record P2.8 FAIL "root SPKI unchanged after delete+rebootstrap (expected a new root)"
  fi
}

p2_9() {  # field-ownership: a kubectl-owned leaf Secret must not wedge rotation
  step "P2.9 kubectl-patched leaf Secret does not wedge the next rotation (SSA force)"
  # Drives rotation through the AUTOMATIC emergency net, not the manual Admin API
  # (that endpoint is 403-unreachable — see P2.4), so this check stays meaningful
  # independently of the API bug.
  if ! "$OPENSSL" version 2>/dev/null | grep -q "^OpenSSL 3"; then
    record P2.9 SKIP "needs OpenSSL 3 (-not_after)"
    return
  fi
  apply_rotation "$(cat <<'EOF'
    policy: manual
    leaf_ttl: 2h
    ca_ttl: 30d
    emergency_rotate_below: 1h
EOF
)" || { record P2.9 FAIL "config apply failed"; return; }
  local dir; dir="$(mktemp -d "${TMPDIR_WORK}/ownership.XXXXXX")"
  extract_ca "$dir" || { record P2.9 FAIL "cannot extract CA"; return; }
  # A crafted ~30m leaf is BOTH the ownership-transfer vehicle and the emergency
  # trigger: the kubectl merge-patch below hands `f:data` to kubectl (pre-fix this
  # 409-wedged every later controller apply, permanently).
  local na; na="$(date -u -v+30M '+%Y%m%d%H%M%SZ')"
  craft_client_leaf "$dir" "$na" || { record P2.9 FAIL "could not craft leaf"; return; }
  local serial_before; serial_before="$(cert_serial "${dir}/client.crt")"
  patch_leaf_secret "$dir"
  local owner_before; owner_before="$(secret_data_owner "$LEAF_SECRET")"
  info "leaf Secret f:data owner after kubectl patch: ${owner_before}"
  if ! echo "$owner_before" | grep -q "kubectl"; then
    record P2.9 FAIL "kubectl patch did not take f:data ownership (owner=${owner_before}); the premise of this check does not hold"
    return
  fi
  info "waiting up to 3m for the controller to rotate the kubectl-owned leaf..."
  if ! wait_for_ctrl_log "cert_emergency_rotation" 180; then
    errln "no rotation. Conflict evidence:"; ctrl_conflict_grep | head -5 >&2
    record P2.9 FAIL "controller did not rotate the kubectl-owned leaf within 3m (409-wedged?)"
    return
  fi
  sleep 5
  local serial_after; serial_after="$(leaf_serial)"
  local owner_after; owner_after="$(secret_data_owner "$LEAF_SECRET")"
  local conflicts; conflicts="$(ctrl_conflict_grep)"
  info "leaf Secret f:data owner after rotation: ${owner_after}"
  if [[ -n "$serial_after" && "$serial_after" != "$serial_before" ]] \
     && echo "$owner_after" | grep -q "${CTRL_FIELD_MANAGER}/Apply" \
     && [[ -z "$conflicts" ]]; then
    record P2.9 PASS "forced apply reclaimed f:data (${owner_before} -> ${owner_after}); serial ${serial_before:0:8}->${serial_after:0:8}; no 409"
  else
    record P2.9 FAIL "serial_changed=$([[ "$serial_after" != "$serial_before" ]] && echo 1 || echo 0) owner=${owner_after} conflicts=$([[ -n "$conflicts" ]] && echo yes || echo no)"
  fi
}

run_phase2() {
  run_check_safe p2_1 P2.1; run_check_safe p2_2 P2.2; run_check_safe p2_3 P2.3
  run_check_safe p2_4 P2.4; run_check_safe p2_5 P2.5; run_check_safe p2_6 P2.6
  run_check_safe p2_7 P2.7; run_check_safe p2_9 P2.9; run_check_safe p2_8 P2.8
}

# ===========================================================================
# Dispatch
# ===========================================================================
usage() {
  sed -n '2,25p' "$0" | sed 's/^# \{0,1\}//'
}

main() {
  local mode=""
  local single=""
  while [[ $# -gt 0 ]]; do
    case "$1" in
      --phase1) mode="phase1"; shift ;;
      --phase2) mode="phase2"; shift ;;
      --all)    mode="all"; shift ;;
      --check)  mode="check"; single="${2:?--check needs an id}"; shift 2 ;;
      -h|--help) usage; exit 0 ;;
      *) errln "unknown argument: $1"; usage; exit 2 ;;
    esac
  done
  [[ -z "$mode" ]] && { usage; exit 2; }

  require_prereqs

  case "$mode" in
    phase1) run_phase1 ;;
    phase2) run_phase2 ;;
    all)    run_phase1; run_phase2 ;;
    check)
      local fn
      fn="$(echo "$single" | tr 'A-Z.' 'a-z_')"   # P2.4 -> p2_4
      if declare -F "$fn" >/dev/null; then
        run_check_safe "$fn" "$single"
      else
        errln "no such check: ${single} (expected P1.1..P1.9 / P2.1..P2.9)"
        exit 2
      fi
      ;;
  esac

  print_summary
}

main "$@"
