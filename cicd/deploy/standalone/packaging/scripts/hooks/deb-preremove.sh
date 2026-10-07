case "${1:-}" in
    upgrade|deconfigure) capture_and_stop ;;
    remove) erase_service ;;
    failed-upgrade) capture_and_stop ;;
    *) fail "Unsupported prerm action: ${1:-}" ;;
esac
