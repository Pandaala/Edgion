case "${1:-}" in
    remove|purge) finalize_removal ;;
    abort-install|abort-upgrade|failed-upgrade|upgrade|disappear) : ;;
    *) fail "Unsupported postrm action: ${1:-}" ;;
esac
