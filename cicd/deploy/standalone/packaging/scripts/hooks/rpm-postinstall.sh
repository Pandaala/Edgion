layout
# Restart waits until the old package has completed its uninstall scriptlets.
if systemd_running; then systemctl daemon-reload; fi
