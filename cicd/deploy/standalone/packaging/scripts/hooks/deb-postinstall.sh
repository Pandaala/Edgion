case "${1:-}" in
    configure) layout configure; restore_removal; resume ;;
    abort-upgrade|abort-remove|abort-deconfigure) layout configure; restore_removal; resume ;;
    *) fail "Unsupported postinst action: ${1:-}" ;;
esac
