# Reproduces the `aarch64-unknown-linux-gnu` release cross-compile environment
# locally, to catch C-runtime portability breaks before they reach CI.
#
# The base image is the SAME one `cross` uses for that target: Ubuntu 16.04
# with the aarch64-linux-gnu GCC toolchain and a matching glibc 2.23 sysroot
# baked in. <sys/random.h> (glibc >= 2.25) is absent here, exactly as on the
# CI cross runner — so a regression that includes it from a shared header
# fails this build the same way it failed CI.
#
# Usage (from the repo root):
#   podman build -f ci/cross-aarch64/Dockerfile -t ruxen-cross-aarch64 .
#   # or: docker build -f ci/cross-aarch64/Dockerfile -t ruxen-cross-aarch64 .
#
# A successful build == every stdlib runtime .c cross-compiles for aarch64.
# This is a fast, low-memory check (compile-only, no Rust, no linking); it is
# NOT a full `cross build` of the `ruxen` binary, but it covers the exact
# layer that broke in the 0.1.0 release run.
FROM ghcr.io/cross-rs/aarch64-unknown-linux-gnu:0.2.5

WORKDIR /work

# Only the C runtime + the check script are needed; copying just these keeps
# the build context small and the image cache stable across Rust-only edits.
COPY library/ library/
COPY ci/cross-aarch64/compile-runtime.sh /usr/local/bin/compile-runtime.sh

RUN sh /usr/local/bin/compile-runtime.sh
