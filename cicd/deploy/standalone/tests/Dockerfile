ARG BASE=debian:bookworm-slim
FROM ${BASE}
ENV container=docker
RUN if command -v apt-get >/dev/null; then \
      apt-get update && DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends \
        systemd systemd-sysv dbus ca-certificates curl python3 binutils procps iproute2 libgcc-s1 libstdc++6 && \
      rm -rf /var/lib/apt/lists/*; \
    else \
      dnf install -y systemd dbus ca-certificates python3 binutils procps-ng iproute libgcc libstdc++ shadow-utils diffutils && \
      dnf clean all; \
    fi; \
    rm -f /usr/sbin/policy-rc.d
STOPSIGNAL SIGRTMIN+3
CMD ["/sbin/init"]
