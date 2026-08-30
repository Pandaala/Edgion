#!/usr/bin/env bash
# ============================================================================
# rekey_conf_sync_ca.sh
#
# EMERGENCY: rotate the conf_sync CA ROOT PRIVATE KEY (compromise response).
#
# WHEN TO USE THIS
#   A conf_sync private key leaked (the root in `edgion-conf-sync-ca`, or a leaf
#   in `edgion-conf-sync-certs`). Re-keying is the ONLY true revocation: every
#   leaf ever signed by the old root stops validating the instant the new root
#   replaces it.
#
# WHEN *NOT* TO USE THIS
#   Routine rotation. The reconciler already rotates leaves on its own, and the
#   `scope=ca` Admin API performs a SAME-KEY root renewal — which extends the
#   root's validity but revokes NOTHING (the public key is unchanged, so every
#   old leaf still verifies). Reach for this script only for compromise.
#
# WHAT IT DOES
#   Deletes both Secrets. The elected leader's next reconciler tick (<=60s) finds
#   the leaf Secret gone, re-bootstraps a BRAND-NEW root (new private key),
#   re-signs both leaves, and hot-swaps its own serving identity. Every OTHER
#   controller replica re-signs its own serving identity — and, crucially, the
#   client-CA verifier that authenticates gateways — from the per-replica
#   identity reconciler (`identity_sync.rs`, 60s, runs leader or not). Gateways
#   pick the new material up from their Secret mount (kubelet sync + a 2s
#   debounce). No process restarts are required: the leader reconciler's
#   trust-root consistency check (`out_of_sync_reason`) also self-heals a leader
#   failover that lands on a replica still holding a pre-rekey in-memory identity.
#
# ROTATION vs. RE-KEY — the distinction the whole design turns on
#   * A routine ROTATION (same trust root, new leaf) deliberately does NOT disturb
#     established conf_sync sessions. The peers on them are still authorized; the
#     zero-disruption property is the point.
#   * A RE-KEY (this script — the trust ROOT changes) DOES tear established sessions
#     down. That is what makes it revocation. Certificates are consumed once, at
#     handshake, so swapping the client-CA verifier alone would only govern FUTURE
#     handshakes: an attacker with a leaked leaf and an already-open HTTP/2 connection
#     would keep issuing List/Watch and keep receiving the resolved-Secret stream
#     forever, without ever re-authenticating. Each controller therefore scopes every
#     accepted connection to the trust root that authenticated it and kills those
#     connections when the root changes (`tls_identity.rs` trust generation +
#     `grpc_server.rs` TrustScopedStream). Still NO process restarts required.
#
# WHAT IT PROVES (this is a compromise-response tool; a green run must MEAN
# something)
#   POSITIVE — the new root works everywhere:
#     * the root in the Secret has a NEW public key (SPKI changed — not a same-key
#       renewal, which revokes nothing),
#     * the gateway leaf Secret chains to it,
#     * every GATEWAY's MOUNTED leaf chains to it (read off the pod, restart-proof),
#     * every CONTROLLER pod actually SERVES a leaf chaining to it (port-forward +
#       `openssl s_client`), and logged a NEW rotation / identity re-sign.
#   NEGATIVE — the OLD key no longer works (the only true proofs of revocation):
#     * NEW handshakes: the pre-re-key client cert+key are captured BEFORE the delete,
#       and after propagation an mTLS handshake with them must be REJECTED by EVERY
#       controller pod.
#     * LIVE sessions: an mTLS session opened with the OLD client identity BEFORE the
#       delete and held open ACROSS the re-key must be TORN DOWN. This is the
#       attacker's actual position — they do not need to re-handshake — and it is the
#       one thing every other check in this script is blind to.
#   Any of these failing exits non-zero. "Cannot check" never reads as "passed": a
#   dead port-forward, unreadable logs, or an uncaptured old identity are all
#   INCONCLUSIVE, and inconclusive is not a pass.
#
# DISRUPTION (read before running)
#   conf_sync mTLS handshakes FAIL during the propagation window (typically
#   ~1-2 min: <=60s for the leader's tick, plus kubelet's Secret sync), and every
#   established conf_sync session is torn down (see above — that is the revocation).
#   Gateways reconnect and relist. The DATA PLANE is unaffected throughout — gateways
#   keep serving traffic from their in-memory config. What pauses is control-plane
#   config propagation. Each gateway raises
#   `edgion_system_error_state{reason="confsync_disconnected"}` on ITS OWN metrics
#   listener while its link is down and clears it on reattach; that set-then-clear is
#   expected, not a failure. A gauge that never clears IS a failure — that gateway did
#   not come back and is serving frozen config.
#
# FILE / SOLO MODE (auto_ca: false)
#   Not applicable — this script drives the Kubernetes auto-CA path. In file mode
#   you re-key by regenerating the CA + both leaves yourself and replacing the
#   files on BOTH sides; each side hot-reloads within ~2s of its files changing.
#
# Usage:
#   rekey_conf_sync_ca.sh              # dry run: show the plan and current state
#   rekey_conf_sync_ca.sh --yes        # actually re-key, then verify
#   rekey_conf_sync_ca.sh --verify     # re-run the post-conditions only
#
# SAFETY: only ever touches the `edgion-system` namespace.
# Runbook prose: skills/02-features/02-config/00-controller-config.md
#                ("Emergency: CA root re-key").
# ============================================================================
set -euo pipefail

readonly NS="edgion-system"
readonly CA_SECRET="edgion-conf-sync-ca"
readonly LEAF_SECRET="edgion-conf-sync-certs"
readonly CTRL_LABEL="app=edgion-controller"
readonly GW_LABEL="app=edgion-gateway"
readonly LOG_GLOB="/usr/local/edgion/logs/*.log"
# The leader's tick is 60s; kubelet's Secret refresh adds up to its sync period.
readonly ROOT_TIMEOUT=180
# The reconciler writes the CA Secret before the leaf (ensure_ca, then write_leaf),
# so the leaf legitimately lags the root by moments.
readonly LEAF_TIMEOUT=120
readonly GW_TIMEOUT=240
# Controller-side proofs: each replica's own identity reconciler ticks every 60s,
# so allow a couple of ticks plus port-forward setup.
readonly CTRL_TIMEOUT=240
# Where the gateway deployment mounts edgion-conf-sync-certs (gateway/deployment.yaml).
readonly GW_MOUNTED_LEAF="/etc/edgion/conf-sync-certs/client.crt"
# The Service that fronts the controller's conf_sync gRPC port; we read the port
# number from it rather than hardcoding 12151.
readonly LEADER_SVC="edgion-controller-leader"
# How long the old-key probe holds the connection open after its handshake, waiting
# for the server's verdict. Non-negotiable under TLS 1.3: the rejection alert arrives
# AFTER the client's handshake completes (see old_key_handshake). 3s is far beyond an
# in-cluster round trip through a port-forward.
readonly HANDSHAKE_LINGER=3

# How long the pre-delete "attacker" session is held open (seconds). It must outlive
# the whole propagation + verification window, or its own expiry would masquerade as a
# teardown. Its feeder `sleep` is what keeps s_client's stdin open.
readonly LIVE_SESSION_HOLD=1800
# How long we wait, after the re-key, for that live session to be torn down. The
# non-leader replicas react on their own 60s identity_sync tick.
readonly LIVE_SESSION_TIMEOUT=240

TMPDIR_REKEY="$(mktemp -d)"
PF_PID=""
PF_PORT=""
# The long-lived "attacker" session held across the re-key (see live_session_start).
LIVE_POD=""
LIVE_PF_PID=""
LIVE_PF_PORT=""
LIVE_SC_PID=""
# Always tear the port-forwards down, on every exit path (including `die`/set -e).
trap 'stop_pf; live_session_stop; rm -rf "$TMPDIR_REKEY"' EXIT

kc() { kubectl -n "$NS" "$@"; }

c_red()   { printf '\033[31m%s\033[0m\n' "$*"; }
c_green() { printf '\033[32m%s\033[0m\n' "$*"; }
c_bold()  { printf '\033[1m%s\033[0m\n' "$*"; }
step()    { printf '\n\033[1m==> %s\033[0m\n' "$*"; }
die()     { c_red "ERROR: $*"; exit 1; }

# secret_field <secret> <key>: data keys contain dots (ca.crt), which must be
# escaped in jsonpath or it reads them as nested fields and returns "".
secret_field() {
  local sec="$1" key="$2" raw esc
  esc="${key//./\\.}"
  raw="$(kc get secret "$sec" -o jsonpath="{.data.${esc}}" 2>/dev/null || true)"
  [[ -z "$raw" ]] && return 0
  echo "$raw" | base64 -d 2>/dev/null || true
}

# spki_of <pem-file>: sha256 of the DER SubjectPublicKeyInfo. This — not the
# cert fingerprint — is what distinguishes a real re-key (new PRIVATE KEY, new
# SPKI) from a same-key renewal (new cert, SAME SPKI, revokes nothing).
#
# Fails (returns 1, prints nothing) when the PEM is missing or unparsable. This
# matters: hashing an EMPTY pipeline would yield the sha256-of-nothing constant
# (e3b0c442...), which is non-empty and differs from any real old SPKI — i.e. a
# corrupt new root would sail through the "SPKI changed" assertion as a PASS.
spki_of() {
  local pem="$1" der
  der="$(openssl x509 -in "$pem" -pubkey -noout 2>/dev/null \
         | openssl pkey -pubin -outform DER 2>/dev/null | base64)" || return 1
  [[ -n "$der" ]] || return 1
  printf '%s' "$der" | base64 -d | openssl dgst -sha256 -r 2>/dev/null | awk '{print $1}'
}

ctrl_pods() { kc get pods -l "$CTRL_LABEL" -o jsonpath='{.items[*].metadata.name}' 2>/dev/null || true; }
gw_pods()   { kc get pods -l "$GW_LABEL"   -o jsonpath='{.items[*].metadata.name}' 2>/dev/null || true; }

# Sentinel count meaning "we could not read this pod's logs at all". Distinct from
# 0 ("read them; no matches") and from -1 ("no baseline"), because the difference is
# the difference between a proof and a guess: swallowing an `exec` failure into 0 made
# the caller's `now < base` arm fire and print the reassuring "pod restarted" line
# while, in fact, we never managed to look.
readonly LOG_UNREADABLE=-2

# Both binaries log to files (console:false), so `kubectl logs` is empty.
#
# Returns 2 (printing nothing) when we could not LOOK — `kubectl exec` denied, pod
# gone, api-server hiccup. The remote `|| true` makes grep's own "no match" (exit 1)
# an empty-but-successful read, so the caller can tell "zero occurrences" from
# "could not read the logs". Swallowing the latter into a count of 0 is what let the
# caller conclude "the pod restarted, all good" without ever reading a log line.
pod_log_grep() {
  local pod="$1" pattern="$2" out
  out="$(kc exec "$pod" -- sh -c 'grep -F -- "$1" '"$LOG_GLOB"' 2>/dev/null || true' _ "$pattern" 2>/dev/null)" || return 2
  printf '%s\n' "$out"
}
# Log files accumulate for the pod's whole life, so a bare grep also matches
# events from BEFORE this re-key and would report success without anything having
# happened. Count occurrences instead and require the count to GROW past a
# baseline taken before the Secrets were deleted.
# Echoes LOG_UNREADABLE if ANY pod's logs could not be read.
log_count() {
  local pattern="$1" pods="$2" p total=0 n out
  for p in $pods; do
    out="$(pod_log_grep "$p" "$pattern")" || { echo "$LOG_UNREADABLE"; return 0; }
    n="$(printf '%s' "$out" | grep -c . || true)"
    total=$(( total + n ))
  done
  echo "$total"
}

fetch_root() {  # -> writes the current root cert to $1, or returns 1 if absent
  local out="$1" pem
  pem="$(secret_field "$CA_SECRET" "ca.crt")"
  [[ -z "$pem" ]] && return 1
  printf '%s' "$pem" > "$out"
}

# ---------------------------------------------------------------------------
# Controller-side live-handshake proofs.
#
# The Secret contents and the gateways' mounted files say what SHOULD be in use.
# Only a real handshake against each controller says what IS in use — and the
# whole point of the per-replica identity reconciler is that a NON-leader's
# in-memory trust root can silently lag. Deleting that reconciler must make this
# script fail; nothing else in the suite notices.
# ---------------------------------------------------------------------------

# The conf_sync gRPC container port, read from the leader Service (targetPort when
# it is numeric, else the Service port — they are equal today).
ctrl_grpc_port() {
  local tp p
  tp="$(kc get svc "$LEADER_SVC" -o jsonpath='{.spec.ports[?(@.name=="grpc")].targetPort}' 2>/dev/null || true)"
  p="$(kc get svc "$LEADER_SVC"  -o jsonpath='{.spec.ports[?(@.name=="grpc")].port}'       2>/dev/null || true)"
  if [[ "$tp" =~ ^[0-9]+$ ]]; then echo "$tp"; else echo "$p"; fi
}

# A local TCP port nothing is listening on (bash /dev/tcp probe: a *failed* connect
# means free). Ephemeral-range collisions are still theoretically possible; a
# failed port-forward simply retries within the caller's deadline.
free_port() {
  local p i
  for (( i = 0; i < 50; i++ )); do
    p=$(( 20000 + RANDOM % 10000 ))
    if ! (exec 3<>"/dev/tcp/127.0.0.1/$p") 2>/dev/null; then echo "$p"; return 0; fi
  done
  return 1
}

# start_pf <pod> <remote-port>: brings up a port-forward and sets the globals
# PF_PORT (the local port) and PF_PID. Returns 1 if the forward never came up —
# the caller retries and treats that as INCONCLUSIVE, never as a pass.
#
# Deliberately sets globals instead of echoing the port: `lp=$(start_pf ...)` would
# run it in a SUBSHELL, so PF_PID would be lost to the parent and every
# port-forward would leak for the life of the script.
start_pf() {
  local pod="$1" rport="$2" i
  PF_PORT=""
  PF_PORT="$(free_port)" || return 1
  kubectl -n "$NS" port-forward "pod/$pod" "${PF_PORT}:${rport}" >/dev/null 2>&1 &
  PF_PID=$!
  for (( i = 0; i < 40; i++ )); do
    if (exec 3<>"/dev/tcp/127.0.0.1/$PF_PORT") 2>/dev/null; then return 0; fi
    kill -0 "$PF_PID" 2>/dev/null || { PF_PID=""; return 1; }
    sleep 0.25
  done
  stop_pf
  return 1
}

stop_pf() {
  [[ -z "${PF_PID:-}" ]] && return 0
  kill "$PF_PID" 2>/dev/null || true
  wait "$PF_PID" 2>/dev/null || true
  PF_PID=""
}

# served_cert <local-port> <outfile>: capture the cert the server PRESENTS.
#
# This works without a client cert even though the channel is mTLS: the server
# sends its own certificate BEFORE it demands the client's, so the handshake gets
# far enough to hand us the leaf and only then fails. That is exactly how the
# per-replica identity fix was proven by hand.
served_cert() {
  local lp="$1" out="$2" text
  text="$(openssl s_client -connect "127.0.0.1:${lp}" -showcerts </dev/null 2>/dev/null || true)"
  # First cert in the chain = the server's own leaf.
  awk '/-----BEGIN CERTIFICATE-----/{f=1} f{print} /-----END CERTIFICATE-----/{if(f) exit}' <<<"$text" > "$out"
  [[ -s "$out" ]] || return 1
}

# old_key_handshake <local-port>: try to authenticate with the PRE-RE-KEY client
# identity.
#   0 = the server ACCEPTED it   (BAD: the compromise is NOT contained)
#   1 = the server REJECTED it   (GOOD: revocation is real on this pod)
#   2 = INCONCLUSIVE — we could not get a verdict (no TLS server reached, or an
#       ambiguous transport error). Callers must treat this as a FAILURE to prove
#       revocation, never as revocation.
#
# Two traps, both verified against a strict mTLS `openssl s_server`:
#
#  1. DO NOT feed s_client `</dev/null`. Under TLS 1.3 the client's handshake
#     completes before the server has even looked at the client certificate; the
#     server's `unknown ca` alert arrives AFTER. An s_client that closes immediately
#     therefore exits 0 with no alert in its output on a REJECTED connection —
#     indistinguishable from success, and it would make this script scream
#     "compromise not contained" on a perfectly good re-key. So we hold the
#     connection open for HANDSHAKE_LINGER seconds (a `sleep` on stdin) and let the
#     alert land.
#  2. Judge on the ALERT, not on the exit code. `Verify return code: 19
#     (self-signed certificate in certificate chain)` appears even on a fully
#     ACCEPTED session — it is the CLIENT's opinion of the SERVER's cert (we pass no
#     -CAfile, and we do not care: proof #1 already verified the served leaf). Only
#     an explicit TLS alert (`tlsv1 alert unknown ca`, `SSL alert number 48`, ...)
#     means the server threw us out.
#
# Anything else with a non-zero exit (connection reset, unexpected EOF, ...) is
# INCONCLUSIVE, not "rejected": failing closed here is the whole point — a transport
# hiccup must never be recorded as proof that a leaked key was revoked.
old_key_handshake() {
  local lp="$1" out rc=0
  out="$(sleep "$HANDSHAKE_LINGER" | openssl s_client -connect "127.0.0.1:${lp}" -showcerts \
           -cert "$TMPDIR_REKEY/old-client.crt" -key "$TMPDIR_REKEY/old-client.key" 2>&1)" || rc=$?
  # No server certificate => we never spoke TLS to anything. Inconclusive.
  grep -q -- "-----BEGIN CERTIFICATE-----" <<<"$out" || return 2
  # An explicit fatal TLS alert = the server rejected our client certificate.
  if grep -qiE 'SSL alert number|alert unknown ca|alert bad certificate|alert certificate|alert handshake failure|alert unsupported certificate|:fatal:' <<<"$out"; then
    return 1
  fi
  # Clean session, no alert => the old key still authenticates.
  (( rc == 0 )) && return 0
  return 2
}

# Per-pod count of "this controller re-signed against a new root" events. The
# leader logs conf_sync_cert_rotated; a NON-leader logs
# conf_sync_server_identity_resigned (identity_sync.rs) and never the former —
# summing across pods, as this script used to, let one leader's event stand in for
# the whole fleet. Emits "<pod> <count>" lines, with LOG_UNREADABLE for a pod whose
# logs we could not read at all (INCONCLUSIVE — never a pass).
ctrl_event_counts() {
  local p rot res n
  for p in $(ctrl_pods); do
    if ! rot="$(pod_log_grep "$p" "conf_sync_cert_rotated")" \
       || ! res="$(pod_log_grep "$p" "conf_sync_server_identity_resigned")"; then
      printf '%s %s\n' "$p" "$LOG_UNREADABLE"
      continue
    fi
    n=$(( $(printf '%s' "$rot" | grep -c . || true) + $(printf '%s' "$res" | grep -c . || true) ))
    printf '%s %s\n' "$p" "$n"
  done
}

# ---------------------------------------------------------------------------
# The live-session proof: hold an "attacker" mTLS session open ACROSS the re-key.
#
# THIS is the attacker's real position after a leak. They do NOT need to re-handshake:
# an established HTTP/2 connection keeps issuing List/Watch on an already-authenticated
# TLS session, and the resolved Secrets (TLS private keys, backend credentials) keep
# flowing. Every other check in this script opens a NEW connection, so every other
# check is blind to exactly the connection that matters.
#
# We therefore open a session with the OLD client identity BEFORE the delete, confirm it
# established, hold it across the re-key, and afterwards require it to be DEAD.
#
# `openssl s_client` exits as soon as the peer closes the connection, so "the process is
# gone" == "the server tore the session down". The one confound is our own port-forward:
# if kubectl dies, s_client dies too and an outage would masquerade as revocation. So the
# port-forward is checked FIRST, and a dead forward is INCONCLUSIVE, never a pass.
# ---------------------------------------------------------------------------

# live_session_start <grpc-port>: 0 = session established and held; 1 = could not
# establish one (caller treats that as inconclusive, not as proof of anything).
live_session_start() {
  local rport="$1" i out
  LIVE_POD="$(ctrl_pods | awk '{print $1}')"
  [[ -n "$LIVE_POD" ]] || return 1
  LIVE_PF_PORT="$(free_port)" || return 1
  kubectl -n "$NS" port-forward "pod/$LIVE_POD" "${LIVE_PF_PORT}:${rport}" >/dev/null 2>&1 &
  LIVE_PF_PID=$!
  for (( i = 0; i < 40; i++ )); do
    (exec 3<>"/dev/tcp/127.0.0.1/$LIVE_PF_PORT") 2>/dev/null && break
    kill -0 "$LIVE_PF_PID" 2>/dev/null || { LIVE_PF_PID=""; return 1; }
    sleep 0.25
  done
  kill -0 "$LIVE_PF_PID" 2>/dev/null || { LIVE_PF_PID=""; return 1; }

  # The feeder `sleep` keeps stdin open, so s_client lingers instead of closing the
  # connection immediately; it outlives the whole verification window.
  out="$TMPDIR_REKEY/live-session.out"
  : > "$out"
  ( sleep "$LIVE_SESSION_HOLD" | openssl s_client -connect "127.0.0.1:${LIVE_PF_PORT}" \
      -cert "$TMPDIR_REKEY/old-client.crt" -key "$TMPDIR_REKEY/old-client.key" \
      >"$out" 2>&1 ) &
  LIVE_SC_PID=$!

  # Confirm it actually established: a server certificate in the transcript, no fatal
  # alert, and the process still alive. (Under TLS 1.3 the server's verdict on OUR
  # certificate arrives after our handshake completes — hence the wait.)
  for (( i = 0; i < 12; i++ )); do
    sleep 1
    kill -0 "$LIVE_SC_PID" 2>/dev/null || { live_session_stop; return 1; }
    if grep -q -- "-----BEGIN CERTIFICATE-----" "$out" 2>/dev/null; then
      if grep -qiE 'SSL alert number|alert unknown ca|alert bad certificate|:fatal:' "$out" 2>/dev/null; then
        live_session_stop
        return 1
      fi
      return 0
    fi
  done
  live_session_stop
  return 1
}

# live_session_alive: 0 = the held session is still up, 1 = it is gone.
live_session_alive() {
  [[ -n "$LIVE_SC_PID" ]] || return 1
  kill -0 "$LIVE_SC_PID" 2>/dev/null
}

# live_session_pf_alive: 0 = our port-forward is still up. If it is NOT, a dead
# s_client proves NOTHING (we killed its transport, not the server).
live_session_pf_alive() {
  [[ -n "$LIVE_PF_PID" ]] || return 1
  kill -0 "$LIVE_PF_PID" 2>/dev/null
}

live_session_stop() {
  if [[ -n "${LIVE_SC_PID:-}" ]]; then
    kill "$LIVE_SC_PID" 2>/dev/null || true
    wait "$LIVE_SC_PID" 2>/dev/null || true
    LIVE_SC_PID=""
  fi
  if [[ -n "${LIVE_PF_PID:-}" ]]; then
    kill "$LIVE_PF_PID" 2>/dev/null || true
    wait "$LIVE_PF_PID" 2>/dev/null || true
    LIVE_PF_PID=""
  fi
}

# verify_live_session_torn_down: the assertion. 0 = the attacker's live session was
# torn down by the re-key; 1 = it survived (compromise NOT contained) or we could not
# tell (INCONCLUSIVE — which is also not a pass).
verify_live_session_torn_down() {
  local waited=0
  step "Proving the OLD client's LIVE session (opened before the re-key) was TORN DOWN (<=${LIVE_SESSION_TIMEOUT}s)"
  while (( waited < LIVE_SESSION_TIMEOUT )); do
    if ! live_session_pf_alive; then
      c_red "  FAIL: the port-forward carrying the held session died, so its fate is UNKNOWN — we"
      c_red "        cannot distinguish 'the server revoked it' from 'we cut its transport'. This is"
      c_red "        NOT proof of revocation. Re-run the check by hand against $LIVE_POD."
      return 1
    fi
    if ! live_session_alive; then
      c_green "  the pre-re-key session on $LIVE_POD was torn down (live revocation is real)"
      return 0
    fi
    sleep 5; waited=$((waited+5))
  done
  c_red "  FAIL: the mTLS session opened with the OLD (compromised) client identity BEFORE the"
  c_red "        re-key is STILL ALIVE after ${LIVE_SESSION_TIMEOUT}s on $LIVE_POD."
  c_red "        THE COMPROMISE IS NOT CONTAINED. An attacker does not need to re-handshake: on an"
  c_red "        established connection they keep issuing List/Watch and keep receiving the full"
  c_red "        resolved-Secret stream (TLS private keys, backend credentials), no matter what the"
  c_red "        new-handshake checks above say."
  c_red "        Expect the controller to cancel these sessions on a trust-root change"
  c_red "        (tls_identity.rs trust generation + grpc_server.rs TrustScopedStream)."
  return 1
}

# count_for <pod> <"pod count" blob> -> the pod's count, or -1 when absent
# (bash 3.2 on macOS has no associative arrays; keep this portable).
count_for() {
  local pod="$1" blob="$2" n
  n="$(awk -v p="$pod" '$1 == p {print $2}' <<<"$blob")"
  [[ -n "$n" ]] && echo "$n" || echo "-1"
}

preflight() {
  command -v kubectl >/dev/null || die "kubectl not found"
  command -v openssl >/dev/null || die "openssl not found"
  # `kubectl -n X get ns` ignores -n, so it would only prove the cluster is up.
  # Name the namespace explicitly.
  kubectl get ns "$NS" >/dev/null 2>&1 \
    || die "namespace $NS not found (wrong kube context? current: $(kubectl config current-context 2>/dev/null))"
  [[ -n "$(ctrl_pods)" ]] || die "no controller pods in $NS — nothing to re-key"
  # Gateways are only REQUIRED for a real re-key: dry-run and --verify must still
  # work on a controller-only or scaled-to-zero cluster.
  if [[ "${1:-}" == "require-gateways" ]]; then
    [[ -n "$(gw_pods)" ]] || die "no gateway pods in $NS — a re-key cannot be verified end to end"
  fi
}

show_state() {
  step "Current state ($NS)"
  if fetch_root "$TMPDIR_REKEY/old-ca.crt"; then
    # `|| true`: an unparsable root must still print the rest of the state (and is
    # itself a reason to re-key), not blank the whole report.
    printf '  root SPKI      : %s\n' "$(spki_of "$TMPDIR_REKEY/old-ca.crt" || echo '<UNPARSABLE>')"
    printf '  root notAfter  : %s\n' "$(openssl x509 -in "$TMPDIR_REKEY/old-ca.crt" -enddate -noout 2>/dev/null | cut -d= -f2 || echo '<UNPARSABLE>')"
  else
    c_red "  $CA_SECRET: ABSENT (already deleted? the leader will re-bootstrap on its next tick)"
  fi
  printf '  controller pods: %s\n' "$(ctrl_pods)"
  printf '  gateway pods   : %s\n' "$(gw_pods)"
}

# verify_controllers: the two live-handshake proofs against EVERY controller pod.
# Returns 0 only if all of them pass. Expects $TMPDIR_REKEY/new-ca.crt to exist, and
# (for the negative proof) $TMPDIR_REKEY/old-client.{crt,key} captured pre-delete.
verify_controllers() {
  local rc=0 p ok deadline served hs last
  local gport; gport="$(ctrl_grpc_port)"
  if [[ ! "$gport" =~ ^[0-9]+$ ]]; then
    c_red "  FAIL: could not read the 'grpc' port from Service $LEADER_SVC — cannot prove what the"
    c_red "        controllers are serving or trusting. Do NOT assume the old key is revoked."
    return 1
  fi

  # -------------------------------------------------------------------------
  # PROOF #1 (positive): every controller pod SERVES a leaf chaining to the NEW root.
  #
  # Under `ha_mode: all-serve` EVERY replica serves the conf_sync API (resolved
  # Secrets: TLS keys, backend credentials), and each holds its OWN in-memory serving
  # identity + client-CA verifier. Nothing in the Secrets, the logs, or the gateways'
  # mounted files can tell you what a given controller pod is actually presenting and
  # trusting. So ask it directly, over the wire. Delete the per-replica identity
  # reconciler (identity_sync.rs) and this step goes red on every non-leader — which
  # is exactly the regression the old script slept through.
  #
  # Polls with a deadline (like the gateway loop): a non-leader picks the new root up
  # on its own 60s tick, so a one-shot read would fail a perfectly good re-key.
  # -------------------------------------------------------------------------
  step "Verifying each CONTROLLER pod SERVES a leaf chaining to the new root (<=${CTRL_TIMEOUT}s)"
  for p in $(ctrl_pods); do
    served="$TMPDIR_REKEY/served-$p.crt"
    ok=0
    deadline=$(( SECONDS + CTRL_TIMEOUT ))
    while (( SECONDS < deadline )); do
      if start_pf "$p" "$gport"; then
        if served_cert "$PF_PORT" "$served" \
           && openssl verify -CAfile "$TMPDIR_REKEY/new-ca.crt" "$served" >/dev/null 2>&1; then
          ok=1; stop_pf; break
        fi
        stop_pf
      fi
      sleep 5
    done
    if (( ok == 1 )); then
      c_green "  $p: serves a leaf chaining to the NEW root"
    else
      c_red "  FAIL: $p never presented a leaf chaining to the new root within ${CTRL_TIMEOUT}s."
      c_red "        This replica is still serving behind its OLD in-memory trust root — under"
      c_red "        ha_mode: all-serve it will KEEP ACCEPTING the compromised gateway leaf and"
      c_red "        keep streaming resolved Secrets to it. The compromise is NOT contained."
      c_red "        Check by hand:"
      c_red "          kubectl -n $NS port-forward pod/$p 15151:$gport &"
      c_red "          openssl s_client -connect 127.0.0.1:15151 -showcerts </dev/null \\"
      c_red "            | openssl verify -CAfile <new-root>"
      rc=1
    fi
  done

  # -------------------------------------------------------------------------
  # PROOF #2 (negative): the OLD client key is REJECTED by every controller pod.
  #
  # THE ONLY TRUE PROOF OF REVOCATION. Every other assertion in this script shows a
  # NEW key started working; none of them show the OLD one STOPPED — which is the
  # entire purpose of a compromise re-key. We captured the pre-re-key client cert+key
  # before deleting the Secret; now we try to use them. A successful handshake means
  # the leaked credential still opens the door.
  #
  # `old_key_handshake` separates "rejected" (1) from "could not connect" (2): a
  # broken port-forward must NEVER be reported as revocation.
  # -------------------------------------------------------------------------
  if [[ ! -s "$TMPDIR_REKEY/old-client.crt" || ! -s "$TMPDIR_REKEY/old-client.key" ]]; then
    c_red "  NOTE: no pre-re-key client cert/key captured (the leaf Secret was already absent, or"
    c_red "        this is a --verify run), so the OLD-KEY-REJECTED proof cannot run. The checks"
    c_red "        above prove the NEW root works; they do NOT prove the OLD key stopped working."
    return $rc
  fi

  step "Proving the OLD (compromised) client key is REJECTED by every controller (<=${CTRL_TIMEOUT}s)"
  for p in $(ctrl_pods); do
    ok=0; last=2   # 2 = inconclusive — never let that read as "rejected"
    deadline=$(( SECONDS + CTRL_TIMEOUT ))
    while (( SECONDS < deadline )); do
      if start_pf "$p" "$gport"; then
        hs=0; old_key_handshake "$PF_PORT" || hs=$?
        stop_pf
        last=$hs
        # 1 = rejected -> done. 0 = still accepted -> this pod may not have ticked yet
        # (<=60s), so keep retrying until the deadline before condemning it.
        if (( hs == 1 )); then ok=1; break; fi
      fi
      sleep 5
    done
    if (( ok == 1 )); then
      c_green "  $p: REJECTS the old client key (revocation is real on this pod)"
    elif (( last == 0 )); then
      c_red "  FAIL: $p STILL ACCEPTS the old client certificate after ${CTRL_TIMEOUT}s."
      c_red "        THE COMPROMISE IS NOT CONTAINED — the leaked key still authenticates against"
      c_red "        this replica and still receives the full resolved-Secret stream."
      c_red "        (Is the per-replica identity reconciler running? identity_sync.rs)"
      rc=1
    else
      c_red "  FAIL: could not reach $p's conf_sync TLS port, so revocation could NOT be proven on"
      c_red "        it. This is 'UNKNOWN', not 'rejected'. Retry, or check RBAC / port-forward."
      rc=1
    fi
  done
  return $rc
}

# verify_rekey <old-spki> <baseline-ctrl-event-blob> <baseline-reloaded-count>
# The baselines make the log assertions honest: a pod's log file survives the
# re-key, so we require NEW occurrences, not merely any occurrence. Pass "" /
# "-1" from --verify, where no baseline was captured (the counts are then only
# reported, not asserted).
verify_rekey() {
  local old_spki="${1:-}" base_ctrl="${2:-}" base_rel="${3:--1}"
  local waited=0 new_spki=""
  local rc=0

  step "Waiting for the leader to re-bootstrap a NEW root (<=${ROOT_TIMEOUT}s)"
  while (( waited < ROOT_TIMEOUT )); do
    if fetch_root "$TMPDIR_REKEY/new-ca.crt"; then
      # `|| true`: an unparsable root yields an empty SPKI, which must keep us
      # waiting (and ultimately FAIL), never satisfy the assertion below.
      new_spki="$(spki_of "$TMPDIR_REKEY/new-ca.crt" || true)"
      if [[ -n "$new_spki" && "$new_spki" != "$old_spki" ]]; then
        c_green "  new root present after ${waited}s"
        printf '  new root SPKI  : %s\n' "$new_spki"
        break
      fi
    fi
    sleep 5; waited=$((waited+5)); printf '  ... %ss\n' "$waited"
  done
  if [[ -z "$new_spki" || "$new_spki" == "$old_spki" ]]; then
    c_red "  FAIL: no NEW root within ${ROOT_TIMEOUT}s (SPKI unchanged or Secret absent)"
    c_red "        -> is a leader elected? check: kubectl -n $NS logs <controller-pod>"
    return 1
  fi

  # The reconciler POST-creates the CA Secret inside `ensure_ca` and only THEN
  # signs and writes the leaf, so the leaf legitimately lags the root by a moment.
  # Reading it once, right after the root appeared, would declare a perfectly good
  # re-key "INCOMPLETE" — the worst possible lie during a live compromise. Poll.
  step "Waiting for the gateway leaf to be re-signed against the new root (<=${LEAF_TIMEOUT}s)"
  local leaf="$TMPDIR_REKEY/client.crt"
  local leaf_ok=0
  waited=0
  while (( waited < LEAF_TIMEOUT )); do
    if secret_field "$LEAF_SECRET" "client.crt" > "$leaf" 2>/dev/null && [[ -s "$leaf" ]] \
       && openssl verify -CAfile "$TMPDIR_REKEY/new-ca.crt" "$leaf" >/dev/null 2>&1; then
      leaf_ok=1
      c_green "  leaf chains to the new root (after ${waited}s)"
      break
    fi
    sleep 5; waited=$((waited+5))
  done
  if (( leaf_ok == 0 )); then
    c_red "  FAIL: $LEAF_SECRET did not get a leaf chaining to the new root within ${LEAF_TIMEOUT}s"
    rc=1
  fi

  # PER POD, not summed. Only the LEADER logs conf_sync_cert_rotated; every other
  # replica re-signs its own identity and logs conf_sync_server_identity_resigned.
  # A sum let the leader's single event stand in for the entire fleet — i.e. it
  # stayed green even with every non-leader still trusting the compromised root.
  # POLL, like every other post-condition here. A non-leader reacts on its OWN
  # identity_sync tick (60s), which is independent of the leader's. Sampling the
  # counts once, right after the root appears, reads a standby that simply has not
  # ticked yet as "did not react" — a FALSE FAILURE, and mid-incident that is the
  # most damaging lie the tool can tell. (Observed for real: the standby's live
  # handshake proved it served the new root while this one-shot check called it
  # unreacted; its `conf_sync_server_identity_resigned` line landed seconds later.)
  step "Checking EVERY controller pod logged a NEW rotation / identity re-sign (<=${CTRL_TIMEOUT}s)"
  local p base now deadline settled unreadable
  for p in $(ctrl_pods); do
    base="$(count_for "$p" "$base_ctrl")"
    if (( base < 0 )); then
      # Covers both "no baseline" (--verify) and "we could not read this pod's logs
      # before the delete" (LOG_UNREADABLE): with no trustworthy baseline there is
      # nothing to assert against.
      printf '  %s: no usable baseline; not asserted\n' "$p"
      continue
    fi
    settled=0; unreadable=0
    deadline=$(( SECONDS + CTRL_TIMEOUT ))
    while (( SECONDS < deadline )); do
      now="$(count_for "$p" "$(ctrl_event_counts)")"
      if (( now == LOG_UNREADABLE )); then
        # We could not LOOK. That is not "the log file reset" and it is certainly not
        # "the pod reacted" — keep retrying, and if it never clears, say so as an
        # INCONCLUSIVE, not as either a pass or a restart.
        unreadable=1
        sleep 10
        continue
      fi
      unreadable=0
      if (( now > base )); then
        c_green "  $p: re-signed against the new root ($base -> $now)"
        settled=1; break
      fi
      if (( now < base )); then
        # The log file itself reset => this pod restarted; it loaded the new root at
        # boot and never logged a re-sign. Not a failure — and the live-handshake
        # assertions below are the authoritative signal anyway.
        printf '  %s: log file reset (pod restarted; it boots onto the new root)\n' "$p"
        settled=1; break
      fi
      sleep 10
    done
    if (( settled == 1 )); then
      continue
    fi
    if (( unreadable == 1 )); then
      c_red "  WARN: could not read $p's log files (kubectl exec failed / RBAC / pod gone), so this"
      c_red "        pod's reaction is UNKNOWN — not confirmed, and NOT a pass. The live handshake"
      c_red "        proofs below are the authoritative signal; judge by those."
    else
      c_red "  FAIL: $p logged no new conf_sync_cert_rotated / conf_sync_server_identity_resigned"
      c_red "        within ${CTRL_TIMEOUT}s, and its log file never reset (so it did not restart)."
      c_red "        This pod has not reacted to the re-key. The live handshake checks below are"
      c_red "        authoritative — if they pass, this is a logging/observability gap, not a"
      c_red "        revocation gap; if they fail too, the compromise is NOT contained."
      rc=1
    fi
  done

  step "Waiting for the gateways to reload onto the new material (<=${GW_TIMEOUT}s)"
  waited=0
  local rel_now="$base_rel"
  while (( waited < GW_TIMEOUT )); do
    rel_now="$(log_count "conf_sync_cert_reloaded" "$(gw_pods)")"
    if (( rel_now == LOG_UNREADABLE )); then
      c_red "  WARN: could not read the gateway log files, so the reload is UNKNOWN (not a pass)."
      c_red "        The mounted-cert assertion below is the authoritative signal."
      break
    fi
    # A gateway that RESTARTED loads the new material at boot and never logs a
    # reload; its fresh log file also resets the count. Treat a Ready gateway with
    # a fresh log file as success, not as a missing reload.
    if (( base_rel >= 0 && rel_now > base_rel )); then
      c_green "  gateway: conf_sync_cert_reloaded ($base_rel -> $rel_now) after ${waited}s"
      break
    fi
    if (( base_rel < 0 )); then
      printf '  conf_sync_cert_reloaded occurrences: %s (no baseline; not asserted)\n' "$rel_now"
      break
    fi
    sleep 10; waited=$((waited+10)); printf '  ... %ss\n' "$waited"
  done
  if (( base_rel >= 0 && rel_now != LOG_UNREADABLE && rel_now <= base_rel )); then
    c_red "  WARN: no NEW conf_sync_cert_reloaded within ${GW_TIMEOUT}s."
    c_red "        If a gateway restarted it picked the material up at boot instead (fresh log"
    c_red "        file, count reset). Judge by readiness below and by the leaf chain check above."
  fi

  # ---------------------------------------------------------------------------
  # THE assertion that actually proves the fleet left the compromised root.
  #
  # Neither of the softer signals can carry this weight:
  #   * the reload LOG is absent whenever a pod restarted instead of hot-reloading;
  #   * gateway READINESS is a boot-time latch (`cache_data.rs::set_ready` only ever
  #     sets `ready = true`, never resets), so a pod that listed once at boot reports
  #     1/1 forever — even with conf_sync mTLS completely broken.
  # So we read the cert the gateway ACTUALLY HAS ON DISK and verify it against the
  # new root. Restart-proof, hot-reload-proof, and it is the real post-condition:
  # if this passes, that pod can no longer speak with the old key.
  # POLL, do not peek once: kubelet refreshes the Secret mount by building a new
  # timestamped directory and atomically flipping the `..data` symlink, and it does
  # so on its own sync period. A single immediate read races that swap and reports a
  # perfectly good re-key as FAILED — which, mid-incident, is arguably worse than a
  # false pass: it tells the operator the compromise is NOT contained. (Observed for
  # real: one pod failed the one-shot read, and a manual retry seconds later showed
  # its mounted leaf chaining to the new root all along.)
  step "Verifying each gateway's MOUNTED cert chains to the new root (restart-proof, <=${GW_TIMEOUT}s)"
  local p mounted deadline ok
  for p in $(gw_pods); do
    mounted="$TMPDIR_REKEY/mounted-$p.crt"
    ok=0
    deadline=$(( SECONDS + GW_TIMEOUT ))
    while (( SECONDS < deadline )); do
      if kc exec "$p" -- cat "$GW_MOUNTED_LEAF" > "$mounted" 2>/dev/null && [[ -s "$mounted" ]] \
         && openssl verify -CAfile "$TMPDIR_REKEY/new-ca.crt" "$mounted" >/dev/null 2>&1; then
        ok=1
        break
      fi
      sleep 5
    done
    if (( ok == 1 )); then
      c_green "  $p: mounted leaf chains to the NEW root"
    else
      # "Cannot verify" must never read as "verified".
      c_red "  FAIL: $p — its mounted leaf never chained to the new root within ${GW_TIMEOUT}s."
      c_red "        Either kubelet has not synced the Secret, this pod's cert watcher is dead,"
      c_red "        or exec is denied. This pod may STILL HOLD pre-re-key material — the"
      c_red "        compromise is not confirmed contained. Check by hand:"
      c_red "          kubectl -n $NS exec $p -- cat $GW_MOUNTED_LEAF | openssl verify -CAfile <new-root>"
      rc=1
    fi
  done

  # Controller-side live proofs (positive + negative). Own function: it is the
  # only part of this script that talks TLS to the pods, and it carries its own
  # port-forward lifecycle.
  verify_controllers || rc=1

  # The attacker's actual position: a session established BEFORE the re-key, never
  # re-handshaked. Only present in --yes runs (it must be opened with the old identity
  # before the Secrets are deleted); --verify has nothing to assert here.
  if [[ -n "$LIVE_SC_PID" ]]; then
    verify_live_session_torn_down || rc=1
  fi

  step "Gateway readiness (weak signal — see above; readiness latches at boot)"
  # Capture first: piping kubectl straight into `grep -q` can SIGPIPE the writer
  # and, under `set -o pipefail`, make the `if` take the WRONG branch. And an
  # EMPTY pod list must not read as "all Ready" (grep -qv on empty input exits 1,
  # which would have printed the green banner with zero gateways running).
  # Read the Ready CONDITION, not the "1/1" column: an injected sidecar (mesh)
  # makes that column 2/2 and would fail every run. Not a proof of anything (see
  # the latch note above) — reported so an operator sees a crash-looping pod.
  local not_ready=""
  for p in $(gw_pods); do
    local cond
    cond="$(kc get pod "$p" -o jsonpath='{.status.conditions[?(@.type=="Ready")].status}' 2>/dev/null || true)"
    if [[ "$cond" == "True" ]]; then
      printf '  %s: Ready\n' "$p"
    else
      printf '  %s: NOT Ready (%s)\n' "$p" "${cond:-unknown}"
      not_ready="yes"
    fi
  done
  if [[ -n "$not_ready" ]]; then
    c_red "  WARN: a gateway pod is not Ready. The mounted-cert assertion above is the"
    c_red "        authoritative signal; a not-Ready pod may simply be restarting."
  fi

  step "Reminder: transient handshake failures are EXPECTED during propagation"
  echo "  Each gateway raises this on its OWN metrics listener while its conf_sync link"
  echo "  is down, and clears it when it reattaches. Confirm EVERY gateway has cleared"
  echo "  once traffic settles — a pod stuck at 1 never reconnected and is serving"
  echo "  frozen config (which is exactly what the Ready check above cannot tell you):"
  echo "    edgion_system_error_state{component=\"conf_sync_client\",reason=\"confsync_disconnected\"} == 0"
  echo "  Scrape the GATEWAY pods (metrics listener, default :12090) — not the"
  echo "  controller; this script's port-forwards only reach the controller, so this"
  echo "  one is on you."
  return $rc
}

main() {
  local mode="dry" expect_old_spki=""
  case "${1:-}" in
    --yes)    mode="rekey" ;;
    --verify) mode="verify"; expect_old_spki="${2:-}" ;;
    ""|--dry-run) mode="dry" ;;
    *) die "unknown argument: $1 (use --yes, --verify [old-spki], or no argument for a dry run)" ;;
  esac

  if [[ "$mode" == "rekey" ]]; then preflight require-gateways; else preflight; fi
  show_state

  if [[ "$mode" == "verify" ]]; then
    # Honest framing: without the pre-delete baselines this canNOT prove a re-key
    # happened. It proves only that a root EXISTS, the leaf chains to it, and the
    # gateways are Ready. Pass the pre-re-key SPKI to also assert the root changed.
    if [[ -n "$expect_old_spki" ]]; then
      echo "  (asserting the current root differs from the supplied pre-re-key SPKI)"
    else
      c_red "  NOTE: no pre-re-key SPKI supplied — this run CANNOT prove a re-key occurred."
      c_red "        It only checks that a root exists, the leaf chains to it, and gateways are Ready."
      c_red "        Pass the old SPKI to assert the root actually changed:"
      c_red "          $0 --verify <old-root-spki>"
    fi
    local rc=0
    # No baselines in --verify mode: the per-pod log counts are reported, not asserted.
    verify_rekey "$expect_old_spki" "" -1 || rc=$?
    exit $rc
  fi

  if [[ "$mode" == "dry" ]]; then
    step "DRY RUN — nothing was changed"
    cat <<EOF
  Re-running with --yes would:
    1. capture the current client identity, and open an mTLS session with it that is
       held OPEN across the re-key (the attacker's position: no re-handshake needed)
    2. kubectl -n $NS delete secret $CA_SECRET $LEAF_SECRET
    3. wait for the leader to mint a BRAND-NEW root (new private key => new SPKI)
    4. verify the gateway leaf was re-signed against it, and that gateways reloaded
    5. prove revocation: the old key is REJECTED on new handshakes by every controller,
       AND the session held from step 1 has been TORN DOWN

  Expect conf_sync mTLS handshake failures for ~1-2 minutes while the new root
  propagates, and every established conf_sync session to be torn down (gateways
  reconnect and relist — that teardown IS the revocation). The data plane keeps
  serving throughout. A routine rotation, by contrast, disturbs nothing.

  This REVOKES every leaf signed by the old root. Use it only for a key
  compromise — routine renewal is automatic and 'scope=ca' is a same-key
  renewal that revokes nothing.
EOF
    exit 0
  fi

  # --- rekey ---
  local old_spki=""
  if fetch_root "$TMPDIR_REKEY/old-ca.crt"; then
    # `|| true`: an UNPARSABLE existing root must not abort the script under
    # `set -e` — that is exactly a situation where you want to re-key. An empty
    # old_spki is handled downstream (any non-empty new SPKI differs from "").
    old_spki="$(spki_of "$TMPDIR_REKEY/old-ca.crt" || true)"
  fi
  # Baselines BEFORE the delete, so the post-conditions assert NEW log events
  # rather than matching ones that predate this re-key. The controller baseline is
  # PER POD (see ctrl_event_counts): only the leader logs a rotation, so a fleet sum
  # would let the leader's single event vouch for replicas that never moved.
  local base_ctrl base_rel
  base_ctrl="$(ctrl_event_counts)"
  base_rel="$(log_count "conf_sync_cert_reloaded" "$(gw_pods)")"

  # Capture the OLD client identity BEFORE deleting the Secret. This is the only
  # chance to get it, and without it the run cannot prove the compromised key stopped
  # working — only that a new one started.
  #
  # A failed capture is NOT survivable as a green run: it takes BOTH revocation proofs
  # (new-handshake-rejected AND live-session-torn-down) off the table, and this script's
  # invariant is that "cannot check" never reads as "passed". The re-key itself still
  # proceeds — mid-compromise you want the root replaced regardless — but the run exits
  # non-zero, and says why.
  local capture_failed=0
  secret_field "$LEAF_SECRET" "client.crt" > "$TMPDIR_REKEY/old-client.crt" 2>/dev/null || true
  secret_field "$LEAF_SECRET" "client.key" > "$TMPDIR_REKEY/old-client.key" 2>/dev/null || true
  if [[ -s "$TMPDIR_REKEY/old-client.crt" && -s "$TMPDIR_REKEY/old-client.key" ]]; then
    c_green "  captured the current (soon-to-be-revoked) client identity for the revocation proofs"
  else
    capture_failed=1
    c_red "  WARN: could not read client.crt/client.key from $LEAF_SECRET (absent or malformed)."
    c_red "        BOTH revocation proofs (old key rejected on a new handshake; the old key's LIVE"
    c_red "        session torn down) are impossible without it. The re-key will still run, but this"
    c_red "        run will exit NON-ZERO: it can only show that a new root started working, never"
    c_red "        that the leaked one stopped."
  fi

  c_bold ""
  c_red "About to DELETE $CA_SECRET and $LEAF_SECRET in namespace $NS."
  c_red "This revokes every leaf signed by the current root: new handshakes with it fail, and"
  c_red "every conf_sync session established under it is torn down (gateways reconnect and"
  c_red "relist). Expect ~1-2 min of conf_sync handshake failures. The data plane keeps serving."
  printf 'Type REKEY to proceed: '
  # `read` at EOF (non-interactive stdin) returns non-zero; under `set -e` that
  # would abort mutely. Say why.
  local answer=""
  read -r answer || die "aborted: no input on stdin (this step is interactive by design)"
  [[ "$answer" == "REKEY" ]] || die "aborted (nothing was changed)"

  # Open the "attacker" session with the OLD identity and hold it across the delete.
  # Deliberately opened HERE — as late as possible before the delete — so the window in
  # which it could die of natural causes (and thereby fake a teardown) is as small as
  # possible.
  local live_started=0
  if (( capture_failed == 0 )); then
    step "Opening an mTLS session with the OLD client identity, to hold ACROSS the re-key"
    local gport; gport="$(ctrl_grpc_port)"
    if [[ "$gport" =~ ^[0-9]+$ ]] && live_session_start "$gport"; then
      live_started=1
      c_green "  held an established session on $LIVE_POD with the soon-to-be-revoked client key"
    else
      c_red "  WARN: could not establish a session with the old client identity, so the LIVE-SESSION"
      c_red "        teardown proof cannot run. This run cannot show what happens to an attacker's"
      c_red "        already-open connection. Exiting non-zero at the end."
    fi
  fi

  step "Deleting both Secrets"
  kc delete secret "$CA_SECRET" "$LEAF_SECRET" --ignore-not-found
  c_green "  deleted"

  # `|| rc=$?`: under `set -e` a bare call that returns non-zero aborts main
  # BEFORE `rc=$?` runs, so the failure banner below would be dead code and the
  # operator would lose the summary exactly when they need it.
  local rc=0
  verify_rekey "$old_spki" "$base_ctrl" "$base_rel" || rc=$?
  # A run that could not run its revocation proofs is NOT a pass, however green the
  # positive checks came out: it never showed that the leaked key stopped working.
  if (( capture_failed == 1 || live_started == 0 )); then
    rc=1
  fi
  if (( rc == 0 )); then
    step "$(c_green "RE-KEY COMPLETE")"
  else
    step "$(c_red "RE-KEY INCOMPLETE — see the failures above. Do NOT assume the old key is revoked.")"
  fi
  exit $rc
}

main "$@"
