#!/usr/bin/env bash
#
# Ruxen uninstaller.
#
# Removes ~/.ruxen and strips the PATH source line from shell rc files.

set -euo pipefail

RUXEN_HOME="${RUXEN_HOME:-$HOME/.ruxen}"

if [ -t 1 ]; then
  BOLD="$(printf '\033[1m')"; GREEN="$(printf '\033[32m')"
  YELLOW="$(printf '\033[33m')"; RESET="$(printf '\033[0m')"
else
  BOLD=""; GREEN=""; YELLOW=""; RESET=""
fi

SOURCE_LINE='. "$HOME/.ruxen/env"'
COMMENT_LINE='# Added by the Ruxen installer'

strip_rc() {
  local rc="$1"
  [ -f "$rc" ] || return 0
  if ! grep -Fq "$SOURCE_LINE" "$rc" 2>/dev/null; then
    return 0
  fi
  local tmp
  tmp="$(mktemp)"
  # Delete the comment line (if present) and the source line.
  grep -Fv "$SOURCE_LINE" "$rc" | grep -Fv "$COMMENT_LINE" > "$tmp"
  mv "$tmp" "$rc"
  echo "${GREEN}${BOLD} ✓${RESET} cleaned $rc"
}

for rc in "$HOME/.bashrc" "$HOME/.bash_profile" "$HOME/.zshrc" "$HOME/.profile"; do
  strip_rc "$rc"
done

if [ -d "$RUXEN_HOME" ]; then
  echo "${YELLOW}${BOLD} !${RESET} removing $RUXEN_HOME"
  rm -rf "$RUXEN_HOME"
  echo "${GREEN}${BOLD} ✓${RESET} removed $RUXEN_HOME"
else
  echo "${YELLOW}${BOLD} !${RESET} $RUXEN_HOME does not exist"
fi

echo
echo "${GREEN}${BOLD}Ruxen uninstalled.${RESET} Open a new shell to refresh PATH."
