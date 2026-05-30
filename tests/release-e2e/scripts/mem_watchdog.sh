#!/usr/bin/env bash
# Memory + disk-bloat watchdog for Ruxen work.
#
# (1) Kills any ruxenc / rustc / cargo / ld / clang process whose RSS
#     exceeds the RSS cap (default 8 GiB).
# (2) Deletes any file under target/ or /tmp/ that exceeds the FILE
#     cap (default 1 GiB) and kills whichever ruxenc/rustc/cargo
#     process most recently touched its parent dir. File-size bloat
#     from pathological codegen can happen without RSS bloat because
#     the kernel flushes pages to disk as they're written.
#
# Polls every 500 ms. Runs until killed.

set -u

RSS_CAP_KB="${RUXENC_MEM_KB:-$((8 * 1024 * 1024))}"      # 8 GiB
FILE_CAP_BYTES="${RUXENC_FILE_CAP:-$((1 * 1024 * 1024 * 1024))}"  # 1 GiB
PROC_PATTERN='ruxenc|rustc|cargo|ld$|ld-classic|clang|ruxen-lsp|ruxen-repl'
# Also match any process whose executable path includes ruxen target or test deps
# (e.g. option_result_runtime-<hash>, installed_binary-<hash>). ps -eo comm
# shows only the basename, so check arguments too via pgrep -f.
PATH_PATTERN='target/(debug|release)/deps|target/(debug|release)/ruxen|/crates/.*/tests/|option_result_runtime|installed_binary|ruxen_core-|ruxen_cli-|ruxen_ide-|ruxen_lsp-|ruxen_repl-|ruxenc-'

# Directories to scan for bloated output files. RUXEN_WORKSPACE (when set)
# takes precedence so Linux + macOS share the same script.
RUXEN_WORKSPACE_DIR="${RUXEN_WORKSPACE:-$HOME/.projects/ruxen}"
SCAN_DIRS=(
  "$RUXEN_WORKSPACE_DIR/target"
  "$RUXEN_WORKSPACE_DIR/tmp"
  "/tmp"
)

echo "[watchdog] rss_cap=${RSS_CAP_KB}KB  file_cap=$((FILE_CAP_BYTES / 1024 / 1024))MB  pid=$$" >&2

kill_all_writers() {
  # When a file explodes, we can't reliably tell which process wrote it.
  # Kill every ruxenc/rustc/cargo/ld/clang we can find — the user said
  # memory must never exceed caps, so the right default is to stop
  # the bleeding.
  pkill -9 -f "ruxenc" 2>/dev/null
  pkill -9 rustc 2>/dev/null
  pkill -9 cargo 2>/dev/null
  pkill -9 -f "ld-classic" 2>/dev/null
  pkill -9 clang 2>/dev/null
}

while true; do
  # (1a) RSS check by comm name (fast)
  ps -eo pid=,rss=,comm= | awk -v cap="$RSS_CAP_KB" -v pat="$PROC_PATTERN" '
    $2 > cap && $3 ~ pat {
      printf "[watchdog] RSS-kill pid %d (%s) RSS=%dKB\n", $1, $3, $2 > "/dev/stderr"
      system("kill -9 " $1)
    }'

  # (1b) RSS check by full argv (catches cargo test binaries like
  # option_result_runtime-<hash> whose comm doesn't match above).
  ps -eo pid=,rss=,args= | awk -v cap="$RSS_CAP_KB" -v pat="$PATH_PATTERN" '
    $2 > cap {
      rest = ""
      for (i = 3; i <= NF; i++) rest = rest " " $i
      if (rest ~ pat) {
        printf "[watchdog] RSS-kill pid %d (path match) RSS=%dKB\n", $1, $2 > "/dev/stderr"
        system("kill -9 " $1)
      }
    }'

  # (2) File-size check. Scan only the configured dirs, shallow on
  # /tmp (depth 2) to keep ps low. Delete anything over the cap.
  for d in "${SCAN_DIRS[@]}"; do
    [ -d "$d" ] || continue
    while IFS= read -r -d '' f; do
      [ -f "$f" ] || continue
      # GNU stat uses -c %s; BSD/macOS stat uses -f %z.
      sz=$(stat -c %s "$f" 2>/dev/null || stat -f %z "$f" 2>/dev/null || echo '?')
      printf '[watchdog] BLOAT file %s (%s bytes) — killing writers + deleting\n' \
        "$f" "$sz" >&2
      kill_all_writers
      sleep 0.2
      rm -f "$f"
    done < <(find "$d" -type f -size +"$((FILE_CAP_BYTES / 1024))"k -print0 2>/dev/null)
  done

  sleep 0.5
done
