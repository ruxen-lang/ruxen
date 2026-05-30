#!/bin/sh
# Cross-compile every Ruxen stdlib C runtime translation unit for
# aarch64-unknown-linux-gnu against the old (glibc 2.23) sysroot baked into
# the `cross` base image. This mirrors the cc-rs invocation in
# src/ruxen_repl/build.rs — the exact step that failed in the 0.1.0 release
# run with `sys/random.h: No such file or directory`.
#
# We compile only (`-c`), no linking: the goal is to prove every TU is
# compilable against the old sysroot, which is precisely where headers like
# <sys/random.h> (glibc >= 2.25) are absent. Exits non-zero on the first
# failing translation unit so `podman build` / `docker build` fails loudly.
#
# Source selection mirrors build.rs::collect_runtime_sources:
#   * library/std/<pkg>/runtime/*.c                       (each a TU)
#   * one level of recursion: library/std/<pkg>/runtime/<vendor>/*.c
#   * EXCLUDING pcre2_printint.c and pcre2_ucptables.c, which are #included
#     by other PCRE2 units and are not standalone TUs.
set -eu

CC="${CC:-aarch64-linux-gnu-gcc}"
STD_ROOT="library/std"
CORE_RT="$STD_ROOT/core/runtime"
PCRE2_DIR="$STD_ROOT/regex/runtime/pcre2"

# Collect TUs: depth 1 (runtime/*.c) + depth 2 (runtime/*/*.c). No deeper,
# matching build.rs's single level of recursion.
SOURCES="$(
    {
        find "$STD_ROOT" -mindepth 3 -maxdepth 3 -path '*/runtime/*.c'
        find "$STD_ROOT" -mindepth 4 -maxdepth 4 -path '*/runtime/*/*.c'
    } | grep -Ev '/(pcre2_printint|pcre2_ucptables)\.c$' | sort
)"

# Include path: shared header dir is enough for the per-package TUs (they
# include "../../core/runtime/runtime.h" by relative path, and core's own
# files use "runtime.h"). PCRE2 units include "config.h"/"pcre2.h" by bare
# name, so the pcre2 dir must be on the path with its build defines.
INCLUDES="-I $CORE_RT"
PCRE2_DEFS=""
PCRE2_WARN=""
if [ -d "$PCRE2_DIR" ]; then
    INCLUDES="$INCLUDES -I $PCRE2_DIR"
    PCRE2_DEFS="-DHAVE_CONFIG_H -DPCRE2_CODE_UNIT_WIDTH=8"
    # Match build.rs's flag_if_supported() suppressions for vendored code.
    PCRE2_WARN="-Wno-sign-compare -Wno-unused-parameter -Wno-implicit-fallthrough"
fi

# Same shape as cc-rs: opt level 2, PIC, section GC.
CFLAGS="-O2 -fPIC -ffunction-sections -fdata-sections"

OBJ_DIR="$(mktemp -d)"
trap 'rm -rf "$OBJ_DIR"' EXIT

n=0
for c in $SOURCES; do
    n=$((n + 1))
    echo "[cc] $c"
    # shellcheck disable=SC2086
    $CC $CFLAGS $PCRE2_WARN $INCLUDES $PCRE2_DEFS -c "$c" -o "$OBJ_DIR/$n.o"
done

echo "OK: cross-compiled $n runtime translation units for aarch64 (glibc 2.23 sysroot)"
