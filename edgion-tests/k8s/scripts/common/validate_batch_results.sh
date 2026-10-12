#!/usr/bin/env bash

# Validate the tagged stdout protocol emitted by run_test_client_batch.sh.
#
# Source this file and call:
#   edgion_validate_k8s_batch_results EXPECTED_SUITES STDOUT_FILE PROCESS_RC
#
# A valid protocol may contain failed suites. On success, callers can inspect:
#   EDGION_K8S_BATCH_TOTAL
#   EDGION_K8S_BATCH_FAILED
#   EDGION_K8S_BATCH_PASSED
#   EDGION_K8S_BATCH_SKIPPED / EDGION_K8S_BATCH_EXECUTED
# Only static broad scans may opt into allow-tls-terminate-skip as argument 4.
#
# The expected-suites file uses one resource|item pair per line. Blank lines and
# lines whose first non-whitespace character is # are ignored.

edgion_tls_terminate_skip_reason() {
    case "$1" in
        'TLSRoute|Basic'|'TLSRoute|ProxyProtocol'|'TLSRoute|StreamPlugins'|'TLSRoute|MultiSNI'|'TLSRoute|BothAbsentParentRef'|'TLSRoute|GeoRestriction')
            printf 'SKIPPED: suite `%s` requires TLSRouteModeTerminate, which is not currently supported\n' "${1%|*}/${1#*|}"
            ;;
        *) return 1 ;;
    esac
}

# This function is also embedded in the Pod's POSIX shell batch script.
edgion_classify_k8s_suite_output() {
    local suite="$1" output="$2" rc="$3" explicit="$4" reason count
    if [ "$rc" -ne 0 ]; then
        echo FAIL
    elif grep -q '^SKIPPED:' "$output"; then
        count="$(grep -c '^SKIPPED:' "$output")"
        if [ "$explicit" = 0 ] && [ "$count" -eq 1 ] \
            && reason="$(edgion_tls_terminate_skip_reason "$suite")" \
            && grep -Fxq "$reason" "$output"; then
            echo SKIP
        else
            echo FAIL
        fi
    else
        echo PASS
    fi
}

edgion_validate_k8s_batch_results() {
    if [ "$#" -lt 3 ] || [ "$#" -gt 4 ]; then
        echo "ERROR: batch validation requires expected-suites, stdout, process-rc, and optional skip policy" >&2
        return 1
    fi

    local expected_file="$1"
    local stdout_file="$2"
    local process_rc="$3"
    local skip_policy="${4:-reject-skip}"
    local line compact resource item suite
    local marker status suite_rc extra
    local expected_count=0 result_count=0 failed_count=0 skipped_count=0 summary_count=0
    local summary_total="" summary_failed="" summary_skipped=""
    local current_suite="" skip_evidence="" skip_markers=0 reason
    local found seen
    local expected_suites=()
    local seen_suites=()

    EDGION_K8S_BATCH_TOTAL=0
    EDGION_K8S_BATCH_FAILED=0
    EDGION_K8S_BATCH_PASSED=0
    EDGION_K8S_BATCH_SKIPPED=0
    EDGION_K8S_BATCH_EXECUTED=0

    case "$skip_policy" in
        reject-skip|allow-tls-terminate-skip) ;;
        *) echo "ERROR: unknown skip policy: $skip_policy" >&2; return 1 ;;
    esac

    if [ ! -f "$expected_file" ]; then
        echo "ERROR: expected-suites file not found: $expected_file" >&2
        return 1
    fi
    if [ ! -f "$stdout_file" ]; then
        echo "ERROR: batch stdout file not found: $stdout_file" >&2
        return 1
    fi
    case "$process_rc" in
        ''|*[!0-9]*)
            echo "ERROR: batch process rc is not a non-negative integer: $process_rc" >&2
            return 1
            ;;
    esac

    while IFS= read -r line || [ -n "$line" ]; do
        compact="$(printf '%s' "$line" | tr -d '[:space:]')"
        [ -n "$compact" ] || continue
        case "$compact" in
            \#*) continue ;;
        esac

        case "$compact" in
            *'|'*) ;;
            *)
                echo "ERROR: malformed expected suite: $line" >&2
                return 1
                ;;
        esac
        resource="${compact%%|*}"
        item="${compact#*|}"
        if [ -z "$resource" ] || [ -z "$item" ] || [ "$item" != "${item#*|}" ]; then
            echo "ERROR: malformed expected suite: $line" >&2
            return 1
        fi
        suite="${resource}|${item}"

        for seen in ${expected_suites[@]+"${expected_suites[@]}"}; do
            if [ "$seen" = "$suite" ]; then
                echo "ERROR: duplicate expected suite: $suite" >&2
                return 1
            fi
        done
        expected_suites+=("$suite")
        expected_count=$((expected_count + 1))
    done < "$expected_file"

    if [ "$expected_count" -eq 0 ]; then
        echo "ERROR: expected-suites file contains no suites" >&2
        return 1
    fi

    while IFS= read -r line || [ -n "$line" ]; do
        case "$line" in
            '@@SUITE_START '*)
                if [ -n "$current_suite" ]; then
                    echo "ERROR: unfinished or duplicate suite start tag" >&2
                    return 1
                fi
                read -r marker current_suite extra <<EOF
$line
EOF
                if [ -n "$extra" ] || [ -z "$current_suite" ]; then
                    echo "ERROR: malformed suite start tag" >&2
                    return 1
                fi
                skip_evidence=""
                skip_markers=0
                ;;
            'SKIPPED:'*)
                if [ -z "$current_suite" ]; then
                    echo "ERROR: skip evidence outside a suite" >&2
                    return 1
                fi
                skip_markers=$((skip_markers + 1))
                skip_evidence="$line"
                ;;
            '@@SUITE_RESULT'*)
                marker=""
                suite=""
                status=""
                suite_rc=""
                extra=""
                read -r marker suite status suite_rc extra <<EOF
$line
EOF
                if [ "$marker" != "@@SUITE_RESULT" ] || [ -z "$suite" ] \
                    || [ -z "$status" ] || [ -z "$suite_rc" ] || [ -n "$extra" ]; then
                    echo "ERROR: malformed suite result tag: $line" >&2
                    return 1
                fi
                if [ -n "$current_suite" ] && [ "$current_suite" != "$suite" ]; then
                    echo "ERROR: suite result does not match its start tag" >&2
                    return 1
                fi

                found=false
                for seen in ${expected_suites[@]+"${expected_suites[@]}"}; do
                    if [ "$seen" = "$suite" ]; then
                        found=true
                        break
                    fi
                done
                if [ "$found" != "true" ]; then
                    echo "ERROR: result tag names an unknown suite: $suite" >&2
                    return 1
                fi

                for seen in ${seen_suites[@]+"${seen_suites[@]}"}; do
                    if [ "$seen" = "$suite" ]; then
                        echo "ERROR: duplicate result tag for suite: $suite" >&2
                        return 1
                    fi
                done

                case "$suite_rc" in
                    ''|*[!0-9]*)
                        echo "ERROR: suite result rc is not a non-negative integer for $suite: $suite_rc" >&2
                        return 1
                        ;;
                esac
                case "$status" in
                    PASS)
                        if [ "$suite_rc" -ne 0 ] || [ "$skip_markers" -ne 0 ]; then
                            echo "ERROR: PASS suite has a non-zero rc for $suite: $suite_rc" >&2
                            return 1
                        fi
                        ;;
                    FAIL)
                        if [ "$suite_rc" -eq 0 ]; then
                            echo "ERROR: FAIL suite has a zero rc: $suite" >&2
                            return 1
                        fi
                        failed_count=$((failed_count + 1))
                        ;;
                    SKIP)
                        if [ "$skip_policy" != allow-tls-terminate-skip ] \
                            || [ "${EDGION_INTEGRATION_EXPLICIT_TARGET:-1}" != 0 ] \
                            || [ "$suite_rc" -ne 0 ] || [ "$current_suite" != "$suite" ] \
                            || [ "$skip_markers" -ne 1 ] \
                            || ! reason="$(edgion_tls_terminate_skip_reason "$suite")" \
                            || [ "$skip_evidence" != "$reason" ]; then
                            echo "ERROR: unsupported or unproven suite skip: $suite" >&2
                            return 1
                        fi
                        skipped_count=$((skipped_count + 1))
                        ;;
                    *)
                        echo "ERROR: invalid suite result status for $suite: $status" >&2
                        return 1
                        ;;
                esac

                seen_suites+=("$suite")
                result_count=$((result_count + 1))
                current_suite=""
                skip_markers=0
                skip_evidence=""
                ;;
            '@@BATCH_SUMMARY'*)
                marker=""
                summary_total=""
                summary_failed=""
                extra=""
                summary_skipped=""
                read -r marker summary_total summary_failed summary_skipped extra <<EOF
$line
EOF
                if [ "$marker" != "@@BATCH_SUMMARY" ] || [ -z "$summary_total" ] \
                    || [ -z "$summary_failed" ] || [ -n "$extra" ]; then
                    echo "ERROR: malformed batch summary tag: $line" >&2
                    return 1
                fi
                summary_count=$((summary_count + 1))
                if [ "$summary_count" -gt 1 ]; then
                    echo "ERROR: duplicate batch summary tag" >&2
                    return 1
                fi
                case "$summary_total" in
                    total=*) summary_total="${summary_total#total=}" ;;
                    *)
                        echo "ERROR: malformed batch summary total: $line" >&2
                        return 1
                        ;;
                esac
                case "$summary_failed" in
                    failed=*) summary_failed="${summary_failed#failed=}" ;;
                    *)
                        echo "ERROR: malformed batch summary failed count: $line" >&2
                        return 1
                        ;;
                esac
                case "$summary_total" in
                    ''|*[!0-9]*)
                        echo "ERROR: batch summary total is not a non-negative integer: $summary_total" >&2
                        return 1
                        ;;
                esac
                case "$summary_failed" in
                    ''|*[!0-9]*)
                        echo "ERROR: batch summary failed count is not a non-negative integer: $summary_failed" >&2
                        return 1
                        ;;
                esac
                if [ -n "$summary_skipped" ]; then
                    case "$summary_skipped" in
                        skipped=*) summary_skipped="${summary_skipped#skipped=}" ;;
                        *) echo "ERROR: malformed skipped count" >&2; return 1 ;;
                    esac
                    case "$summary_skipped" in
                        ''|*[!0-9]*) echo "ERROR: invalid skipped count" >&2; return 1 ;;
                    esac
                else
                    summary_skipped=0
                fi
                ;;
        esac
    done < "$stdout_file"

    if [ -n "$current_suite" ] || [ "$skip_markers" -ne 0 ]; then
        echo "ERROR: unfinished suite output" >&2
        return 1
    fi
    if [ "$summary_count" -ne 1 ]; then
        echo "ERROR: expected exactly one batch summary tag, found $summary_count" >&2
        return 1
    fi
    if [ "$result_count" -ne "$expected_count" ]; then
        for suite in ${expected_suites[@]+"${expected_suites[@]}"}; do
            found=false
            for seen in ${seen_suites[@]+"${seen_suites[@]}"}; do
                if [ "$seen" = "$suite" ]; then
                    found=true
                    break
                fi
            done
            if [ "$found" != "true" ]; then
                echo "ERROR: missing result tag for suite: $suite" >&2
            fi
        done
        echo "ERROR: suite result count mismatch: expected=$expected_count actual=$result_count" >&2
        return 1
    fi
    if [ "$summary_total" -ne "$result_count" ]; then
        echo "ERROR: batch summary total mismatch: summary=$summary_total actual=$result_count" >&2
        return 1
    fi
    if [ "$summary_failed" -ne "$failed_count" ]; then
        echo "ERROR: batch summary failed mismatch: summary=$summary_failed actual=$failed_count" >&2
        return 1
    fi
    if [ "$summary_skipped" -ne "$skipped_count" ]; then
        echo "ERROR: batch summary skipped mismatch" >&2
        return 1
    fi

    if [ "$failed_count" -eq 0 ]; then
        if [ "$process_rc" -ne 0 ]; then
            echo "ERROR: batch process rc must be zero when no suite failed: $process_rc" >&2
            return 1
        fi
    elif [ "$process_rc" -ne 1 ]; then
        echo "ERROR: batch process rc must be one when suites failed: $process_rc" >&2
        return 1
    fi

    EDGION_K8S_BATCH_TOTAL="$result_count"
    EDGION_K8S_BATCH_FAILED="$failed_count"
    EDGION_K8S_BATCH_SKIPPED="$skipped_count"
    EDGION_K8S_BATCH_EXECUTED=$((result_count - skipped_count))
    EDGION_K8S_BATCH_PASSED=$((result_count - failed_count - skipped_count))
    echo "Batch result protocol valid: total=$result_count failed=$failed_count process_rc=$process_rc"
    return 0
}

if [ "${BASH_SOURCE[0]}" = "$0" ]; then
    edgion_validate_k8s_batch_results "$@"
fi
