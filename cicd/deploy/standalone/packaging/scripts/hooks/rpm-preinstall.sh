case "${1:-}" in 1|2) layout; if [ "$1" -gt 1 ]; then capture_and_stop; fi ;; *) fail 'Unsupported RPM install count' ;; esac
