# Local aarch64 cross-compile check

Reproduces the `aarch64-unknown-linux-gnu` release cross-compile environment
locally so C-runtime portability breaks are caught before they reach the
release workflow.

The 0.1.0 release run failed here: the shared runtime header pulled in
`<sys/random.h>` (glibc ≥ 2.25), which is absent from the old Ubuntu-16.04 /
glibc-2.23 sysroot that `cross` uses for this target. Because that header is
included by every runtime translation unit, the first `.c` compiled aborted
the whole build.

## What it does

`Dockerfile` is based on the **same** image `cross` uses for the target
(`ghcr.io/cross-rs/aarch64-unknown-linux-gnu`). It copies `library/` into the
image and runs `compile-runtime.sh`, which cross-compiles (compile-only, no
link) every stdlib runtime translation unit — mirroring
`src/ruxen_repl/build.rs::collect_runtime_sources` and its cc-rs flags. A
successful run prints `OK: cross-compiled N runtime translation units …` and
exits 0; a portability break exits non-zero with the offending `[cc] <file>`
line immediately above the compiler error.

This is a fast, low-memory check focused on the C layer that broke; it is not a
full `cross build` of the `ruxen` binary.

## Run it

From the repository root, any of:

```sh
# One-shot via the image build (the RUN step performs the check):
podman build -f ci/cross-aarch64/Dockerfile -t ruxen-cross-aarch64 .
#   or: docker build -f ci/cross-aarch64/Dockerfile -t ruxen-cross-aarch64 .

# Re-runnable against your live working tree (no rebuild needed between edits):
podman-compose -f ci/cross-aarch64/docker-compose.yml run --rm cross-aarch64
#   or: docker compose -f ci/cross-aarch64/docker-compose.yml run --rm cross-aarch64
```

Use `podman-compose` (the standalone tool), **not** `podman compose` — the
latter delegates to the Docker compose provider, which requires the podman API
socket (`systemctl --user start podman.socket`). `podman-compose` drives the
podman CLI directly and needs no socket.

## Notes

- **SELinux hosts** (Fedora/RHEL): the compose service sets
  `security_opt: [label=disable]` so the read-only bind mount is readable from
  the container. Without it the run fails with
  `find: 'library/std': Permission denied`. The standalone `podman run`
  equivalent needs `--security-opt label=disable` (or a `:z`/`:Z` volume
  suffix — but that relabels your working tree in place, so the harness avoids
  it).
- **Storage**: keep your container engine's storage on a roomy partition
  (e.g. `~/.local/share/containers`), not a small `tmpfs` — the base image is
  ~1 GB.
- Verified: a clean tree compiles **61** translation units (34 per-package +
  27 PCRE2) green; re-injecting `#include <sys/random.h>` into the shared
  header makes the check fail with the exact CI error, confirming it catches
  the regression.
