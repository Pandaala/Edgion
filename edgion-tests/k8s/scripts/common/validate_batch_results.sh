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
#
# The expected-suites file uses one resource|item pair per line. Blank lines and
# lines whose first non-whitespace character is # are ignored.

edgion_validate_k8s_batch_results() {
    if [ "$#" -ne 3 ]; then
        echo "ERROR: batch validation requires expected-suites, stdout, and process-rc arguments" >&2
        return 1
    fi

    local expected_file="$1"
    local stdout_file="$2"
    local process_rc="$3"
    local line compact resource item suite
    local marker status suite_rc extra
    local expected_count=0 result_count=0 failed_count=0 summary_count=0
    local summary_total="" summary_failed=""
    local found seen
    local expected_suites=()
    local seen_suites=()

    EDGION_K8S_BATCH_TOTAL=0
    EDGION_K8S_BATCH_FAILED=0
    EDGION_K8S_BATCH_PASSED=0

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
                        if [ "$suite_rc" -ne 0 ]; then
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
                    *)
                        echo "ERROR: invalid suite result status for $suite: $status" >&2
                        return 1
                        ;;
                esac

                seen_suites+=("$suite")
                result_count=$((result_count + 1))
                ;;
            '@@BATCH_SUMMARY'*)
                marker=""
                summary_total=""
                summary_failed=""
                extra=""
                read -r marker summary_total summary_failed extra <<EOF
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
                ;;
        esac
    done < "$stdout_file"

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
    EDGION_K8S_BATCH_PASSED=$((result_count - failed_count))
    echo "Batch result protocol valid: total=$result_count failed=$failed_count process_rc=$process_rc"
    return 0
}

if [ "${BASH_SOURCE[0]}" = "$0" ]; then
    edgion_validate_k8s_batch_results "$@"
fi
