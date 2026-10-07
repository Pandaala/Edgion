# Embedded into every maintainer script at build time; never sourced on the host.
set -eu
umask 027
role=@ROLE@
account="edgion-$role"
unit="$account.service"
work="/var/lib/edgion/$role"
restart_state="/var/lib/edgion/.packaging/$role.restart"
remove_state="/var/lib/edgion/.packaging/$role.remove-enabled"

fail() { echo "Edgion package: $*" >&2; exit 1; }

immutable_layout() {
@IMMUTABLE_LAYOUT@
}

# GNU stat uses lstat unless -L is specified. Reject symlinks even if dangling.
root_ancestor() {
    [ ! -L "$1" ] && [ -d "$1" ] || fail "Unsafe ancestor: $1"
    [ "$(stat -c %u "$1")" = 0 ] || fail "Non-root ancestor: $1"
    ancestor_mode=$(stat -c %a "$1")
    [ "$((0$ancestor_mode & 0022))" -eq 0 ] || fail "Writable ancestor: $1"
}

directory() {
    directory_path=$1; directory_owner=$2; directory_group=$3; directory_mode=$4
    if [ -e "$directory_path" ] || [ -L "$directory_path" ]; then
        [ ! -L "$directory_path" ] && [ -d "$directory_path" ] || fail "Unsafe directory: $directory_path"
        [ "$(stat -c %u "$directory_path")" = "$(id -u "$directory_owner")" ] || fail "Conflicting owner: $directory_path"
        [ "$(stat -c %g "$directory_path")" = "$(getent group "$directory_group" | cut -d: -f3)" ] || fail "Conflicting group: $directory_path"
        [ "$(stat -c %a "$directory_path")" = "$directory_mode" ] || fail "Conflicting mode: $directory_path"
    else
        install -d -o "$directory_owner" -g "$directory_group" -m "$directory_mode" "$directory_path"
    fi
}

# Existing host ancestors may be read-only (Rocky ships /usr/bin as 0555).
# Validate their safety without changing mode; use 0755 only for missing dirs.
immutable_directory() {
    if [ -e "$1" ] || [ -L "$1" ]; then
        root_ancestor "$1"
        [ "$(stat -c %g "$1")" = 0 ] || fail "Non-root immutable directory group: $1"
    else
        install -d -o root -g root -m 755 "$1"
    fi
}

identity() {
    if ! getent group "$account" >/dev/null; then
        groupadd --system "$account"
    fi
    group_record=$(getent group "$account")
    group_id=$(printf '%s\n' "$group_record" | cut -d: -f3)
    [ "$group_id" -gt 0 ] && [ "$group_id" -lt 1000 ] || fail "Conflicting group: $account"
    if getent group | awk -F: -v gid="$group_id" -v name="$account" '$3 == gid && $1 != name { found=1 } END { exit !found }'; then
        fail "Shared service GID: $account"
    fi
    case "$(printf '%s\n' "$group_record" | cut -d: -f4)" in
        ''|"$account") ;; *) fail "Unexpected group members: $account" ;;
    esac
    if ! getent passwd "$account" >/dev/null; then
        useradd --system --gid "$account" --home-dir "$work" --no-create-home \
            --shell /usr/sbin/nologin "$account"
    fi
    user_record=$(getent passwd "$account")
    user_id=$(printf '%s\n' "$user_record" | cut -d: -f3)
    [ "$user_id" -gt 0 ] && [ "$user_id" -lt 1000 ] || fail "Conflicting user: $account"
    if getent passwd | awk -F: -v uid="$user_id" -v name="$account" '$3 == uid && $1 != name { found=1 } END { exit !found }'; then
        fail "Shared service UID: $account"
    fi
    [ "$(printf '%s\n' "$user_record" | cut -d: -f4)" = "$group_id" ] || fail "Conflicting primary group: $account"
    [ "$(printf '%s\n' "$user_record" | cut -d: -f6)" = "$work" ] || fail "Conflicting home: $account"
    case "$(printf '%s\n' "$user_record" | cut -d: -f7)" in
        /usr/sbin/nologin|/sbin/nologin) ;; *) fail "Interactive account: $account" ;;
    esac
    [ "$(id -G "$account")" = "$group_id" ] || fail "Unexpected supplementary groups: $account"
    # Nobody else may inherit private-key access through the primary group.
    if getent passwd | awk -F: -v gid="$group_id" -v name="$account" '$4 == gid && $1 != name { found=1 } END { exit !found }'; then
        fail "Shared primary group: $account"
    fi
}

layout() {
    for ancestor in /etc /var /var/lib /var/log /usr /usr/bin /usr/share /usr/share/doc /usr/lib /usr/lib/systemd /usr/lib/systemd/system; do
        root_ancestor "$ancestor"
    done
    identity
    immutable_layout
    directory /etc/edgion root root 755
    directory "/etc/edgion/$role" root "$account" 750
    directory "/etc/edgion/$role/certs" root "$account" 750
    directory /var/lib/edgion root root 755
    directory /var/lib/edgion/.packaging root root 700
    directory "$work" root "$account" 1770
    directory "$work/config" root root 755
    directory "$work/runtime" "$account" "$account" 750
    directory "$work/logs" "$account" "$account" 750
    directory /var/log/edgion root root 755
    directory "/var/log/edgion/$role" "$account" "$account" 750
    if [ "$role" = controller ]; then
        directory "$work/resources" "$account" "$account" 750
        if [ -L "$work/config/crd" ]; then
            [ "$(readlink "$work/config/crd")" = /usr/share/edgion-controller/crd ] || fail 'Conflicting CRD lookup symlink'
        elif [ -e "$work/config/crd" ]; then
            fail 'Conflicting CRD lookup path'
        else
            ln -s /usr/share/edgion-controller/crd "$work/config/crd"
        fi
        for share_path in /usr/share/edgion-controller /usr/share/edgion-controller/crd; do
            if [ -e "$share_path" ] || [ -L "$share_path" ]; then root_ancestor "$share_path"; fi
        done
    fi
    # Reject destinations that a package manager could otherwise follow.
    for config_path in "/etc/edgion/$role/edgion-$role.yaml" "/etc/edgion/$role/environment"; do
        if [ -e "$config_path" ] || [ -L "$config_path" ]; then
            [ ! -L "$config_path" ] && [ -f "$config_path" ] || fail "Unsafe configuration file: $config_path"
            [ "$(stat -c %u "$config_path")" = 0 ] || fail "Conflicting config owner: $config_path"
            config_group=$(stat -c %g "$config_path")
            if [ "$config_group" != "$group_id" ]; then
                # DEB tar entries cannot encode a dynamically allocated system GID.
                # Only post-unpack/configure may assign the exact root-owned files.
                [ "${1:-}" = configure ] && [ "$config_group" = 0 ] || fail "Conflicting config group: $config_path"
            fi
            [ "$(stat -c %a "$config_path")" = 640 ] || fail "Conflicting config mode: $config_path"
            if [ "$config_group" != "$group_id" ]; then chown "root:$account" "$config_path"; fi
        fi
    done
    for binary_path in "/usr/bin/$account" "/usr/lib/systemd/system/$unit"; do
        if [ -e "$binary_path" ] || [ -L "$binary_path" ]; then
            [ ! -L "$binary_path" ] && [ -f "$binary_path" ] || fail "Unsafe immutable file: $binary_path"
            [ "$(stat -c %u "$binary_path")" = 0 ] || fail "Non-root immutable file: $binary_path"
        fi
    done
}

systemd_running() { command -v systemctl >/dev/null && [ -d /run/systemd/system ]; }

capture_and_stop() {
    systemd_running || return 0
    directory /var/lib/edgion root root 755
    directory /var/lib/edgion/.packaging root root 700
    if systemctl is-active --quiet "$unit"; then
        if [ -e "$restart_state" ] || [ -L "$restart_state" ]; then
            validate_restart_state
            printf 'active\n' > "$restart_state"
        else
            (umask 077; set -C; printf 'active\n' > "$restart_state")
        fi
        systemctl stop "$unit"
    fi
}

validate_restart_state() {
    root_ancestor /var
    root_ancestor /var/lib
    root_ancestor /var/lib/edgion
    directory /var/lib/edgion/.packaging root root 700
    [ ! -L "$restart_state" ] && [ -f "$restart_state" ] || fail 'Unsafe package restart state'
    [ "$(stat -c %u:%g:%a "$restart_state")" = 0:0:600 ] || fail 'Conflicting package restart state permissions'
    case "$(cat "$restart_state")" in active|deferred) ;; *) fail 'Invalid package restart state' ;; esac
}

resume() {
    systemd_running || return 0
    systemctl daemon-reload
    if [ -e "$restart_state" ] || [ -L "$restart_state" ]; then
        validate_restart_state
        if [ "$(cat "$restart_state")" = deferred ] && [ "${EDGION_PACKAGE_RECOVER:-}" != 1 ]; then
            echo "Edgion package: $unit restart awaits explicit operator recovery" >&2
            return 0
        fi
        case "$(systemctl is-enabled "$unit" 2>/dev/null || true)" in
            masked|masked-runtime) printf 'deferred\n' > "$restart_state"; return 0 ;;
        esac
        if [ "@FORMAT@" = deb ] && [ -x /usr/sbin/policy-rc.d ]; then
            policy_status=0
            /usr/sbin/policy-rc.d "$unit" start || policy_status=$?
            case "$policy_status" in
                0) ;; 101) printf 'deferred\n' > "$restart_state"; echo "Edgion package: native policy defers $unit restart" >&2; return 0 ;;
                *) fail "Native service policy failed for $unit (exit $policy_status)" ;;
            esac
        fi
        unit_active_state=$(systemctl show --property=ActiveState --value "$unit")
        if [ "$unit_active_state" = failed ]; then
            systemctl reset-failed "$unit"
        fi
        systemctl start "$unit"
        rm -f "$restart_state"
    fi
}

erase_service() {
    systemd_running || return 0
    directory /var/lib/edgion root root 755
    directory /var/lib/edgion/.packaging root root 700
    if [ -e "$remove_state" ] || [ -L "$remove_state" ]; then
        validate_remove_state
    else
        removed_enablement=$(systemctl is-enabled "$unit" 2>/dev/null || true)
        case "$removed_enablement" in
            enabled|enabled-runtime|disabled|masked|masked-runtime) ;;
            *) fail "Unsupported removal enablement: $removed_enablement" ;;
        esac
        (umask 077; set -C; printf '%s\n' "$removed_enablement" > "$remove_state")
    fi
    capture_and_stop
    case "$(systemctl is-enabled "$unit" 2>/dev/null || true)" in
        masked|masked-runtime|disabled) ;;
        enabled-runtime) systemctl disable --runtime "$unit" ;;
        *) systemctl disable "$unit" ;;
    esac
}

validate_remove_state() {
    root_ancestor /var
    root_ancestor /var/lib
    root_ancestor /var/lib/edgion
    directory /var/lib/edgion/.packaging root root 700
    [ ! -L "$remove_state" ] && [ -f "$remove_state" ] || fail 'Unsafe package removal state'
    [ "$(stat -c %u:%g:%a "$remove_state")" = 0:0:600 ] || fail 'Conflicting package removal state permissions'
    case "$(cat "$remove_state")" in
        enabled|enabled-runtime|disabled|masked|masked-runtime) ;;
        *) fail 'Invalid package removal state' ;;
    esac
}

restore_removal() {
    systemd_running || return 0
    if [ -e "$remove_state" ] || [ -L "$remove_state" ]; then
        validate_remove_state
        systemctl daemon-reload
        # Restore only enablement changed by our interrupted removal. Never unmask.
        case "$(systemctl is-enabled "$unit" 2>/dev/null || true)" in
            masked|masked-runtime) rm -f "$remove_state"; return 0 ;;
        esac
        case "$(cat "$remove_state")" in
            enabled) systemctl enable "$unit" ;;
            enabled-runtime) systemctl enable --runtime "$unit" ;;
            *) : ;;
        esac
        rm -f "$remove_state"
    fi
}

finalize_removal() {
    if [ -e "$restart_state" ] || [ -L "$restart_state" ]; then
        validate_restart_state
        rm -f "$restart_state"
    fi
    if [ -e "$remove_state" ] || [ -L "$remove_state" ]; then
        validate_remove_state
        rm -f "$remove_state"
    fi
    if systemd_running; then systemctl daemon-reload; fi
}
