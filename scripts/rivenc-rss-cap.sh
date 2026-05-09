#!/usr/bin/env bash
# RSS-cap wrapper for rivenc.
#
# macOS `ulimit -v` does not enforce an address-space cap the way Linux
# does, so we ps-poll the child's resident set size instead. When RSS
# exceeds the configured cap we SIGKILL the child (SIGTERM first is
# pointless — rivenc is compute-bound and won't service signals while
# typechecking a large input) and exit with status 137.
#
# Usage:
#   scripts/rivenc-rss-cap.sh [rivenc-args...]
#
# Env:
#   RIVENC_BIN        Path to rivenc binary. Defaults to rivenc on $PATH.
#   RIVENC_RSS_KIB    RSS cap in KiB. Defaults to 8388608 (8 GiB).
#   RIVENC_POLL_SEC   Poll interval in seconds. Defaults to 1.

set -u

bin="${RIVENC_BIN:-rivenc}"
cap_kib="${RIVENC_RSS_KIB:-8388608}"
poll_sec="${RIVENC_POLL_SEC:-1}"

"$bin" "$@" &
child=$!

trap 'kill -KILL "$child" 2>/dev/null; wait "$child" 2>/dev/null; exit 130' INT TERM

while kill -0 "$child" 2>/dev/null; do
    rss=$(ps -o rss= -p "$child" 2>/dev/null | tr -d ' ')
    if [ -n "$rss" ] && [ "$rss" -gt "$cap_kib" ] 2>/dev/null; then
        printf 'rivenc-rss-cap: RSS %s KiB exceeded cap %s KiB; SIGKILL pid %s\n' \
            "$rss" "$cap_kib" "$child" >&2
        kill -KILL "$child" 2>/dev/null
        wait "$child" 2>/dev/null
        exit 137
    fi
    sleep "$poll_sec"
done

wait "$child"
exit $?
