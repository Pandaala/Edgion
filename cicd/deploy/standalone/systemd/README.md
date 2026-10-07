# Standalone Systemd Services

This directory owns the authoritative [Controller unit](edgion-controller.service),
[Gateway unit](edgion-gateway.service), and optional [same-host drop-in](same-host.conf).
Package builds consume them directly. CLI has no service. Follow the
[deployment guide](../README.md) and [package lifecycle](../packaging/README.md).

Both services run in the foreground as dedicated non-login identities with
absolute config/work paths and protected environment files. Units use
`Restart=on-failure`, five-second retry intervals, and at most five starts in
sixty seconds. SIGTERM selects graceful shutdown in the locked Pingora fork.
Controller allows 45 seconds to stop; Gateway allows 90 seconds, covering its
packaged 30-second grace period, 10-second drain timeout, and auxiliary cleanup.
When increasing configured shutdown budgets, increase `TimeoutStopSec` through
an operator drop-in. There is no `ExecReload` or transparent hot-upgrade contract.

`LimitNOFILE=65536` is per-service. Gateway receives only
`CAP_NET_BIND_SERVICE` for low ports; Controller receives no capabilities.
The units do not change sysctls, PAM limits, or unrelated services. Larger
limits and [optional host tuning](../../linux-tuning/README.md) require explicit
operator action.

Gateway's base unit has no local Controller dependency. For same-host startup,
install `same-host.conf` under `/etc/systemd/system/edgion-gateway.service.d/`
and run `systemctl daemon-reload`. `Wants`/`After` ordering lets Controller
restart independently. Verify readiness separately: ordering does not establish
application availability. Operator overrides under `/etc/systemd/system/`
survive package removal.

First installation does not enable/start either service. Initialize credentials
and resources before starting. Docker verification uses a real systemd PID 1
and container restarts; it does not establish physical-host reboot, bootloader,
kernel, SELinux, or capacity behavior.
