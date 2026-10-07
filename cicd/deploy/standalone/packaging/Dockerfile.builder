# Native builds on the oldest supported libc baseline; no cross-compiler is used.
FROM rockylinux/rockylinux:9
ARG RUST_VERSION=1.96.1
RUN dnf -y install dnf-plugins-core \
    && dnf config-manager --set-enabled crb
RUN dnf -y install gcc gcc-c++ make cmake git clang clang-devel lld \
      perl pkgconf-pkg-config protobuf-compiler openssl-devel \
      tar gzip findutils binutils diffutils ca-certificates \
    && dnf clean all
RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs -o /tmp/rustup-init.sh \
    && sh /tmp/rustup-init.sh -y --profile minimal --default-toolchain "${RUST_VERSION}" \
    && rm /tmp/rustup-init.sh
ENV PATH="/root/.cargo/bin:${PATH}" \
    RUSTFLAGS="-C link-arg=-fuse-ld=lld" \
    CARGO_BUILD_JOBS=1
WORKDIR /source
