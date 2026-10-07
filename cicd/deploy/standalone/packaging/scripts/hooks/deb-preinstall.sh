case "${1:-}" in
    install|upgrade) layout ;;
    abort-upgrade) layout configure ;;
    *) fail "Unsupported preinst action: ${1:-}" ;;
esac
