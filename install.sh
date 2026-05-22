#!/usr/bin/env bash
#
# Riven installer.
#
# Installs the Riven toolchain (riven, rivenc, riven-lsp, riven-repl)
# from GitHub Releases into ~/.riven and configures PATH.
#
# Usage:
#   curl -fsSL https://raw.githubusercontent.com/sherazp995/riven/master/install.sh | bash
#   curl -fsSL https://raw.githubusercontent.com/sherazp995/riven/master/install.sh | bash -s -- --version v0.1.0
#
# Environment overrides:
#   RIVEN_VERSION   Pin a specific release tag (default: latest)
#   RIVEN_REPO      owner/repo on GitHub (default: sherazp995/riven)
#   RIVEN_HOME      Install root (default: $HOME/.riven)
#   RIVEN_NO_MODIFY_PATH=1    Skip editing shell rc files

set -euo pipefail

RIVEN_REPO="${RIVEN_REPO:-sherazp995/riven}"
RIVEN_HOME="${RIVEN_HOME:-$HOME/.riven}"
RIVEN_VERSION="${RIVEN_VERSION:-latest}"
NO_MODIFY_PATH="${RIVEN_NO_MODIFY_PATH:-0}"
# Set to a riven source checkout path to skip GitHub releases and
# build + install from that working tree instead. Cargo on PATH is
# required when this is non-empty.
FROM_SOURCE="${RIVEN_FROM_SOURCE:-}"

# ── ANSI colors (if stdout is a tty) ──────────────────────────────────
if [ -t 1 ]; then
  BOLD="$(printf '\033[1m')"
  DIM="$(printf '\033[2m')"
  RED="$(printf '\033[31m')"
  GREEN="$(printf '\033[32m')"
  YELLOW="$(printf '\033[33m')"
  BLUE="$(printf '\033[34m')"
  RESET="$(printf '\033[0m')"
else
  BOLD=""; DIM=""; RED=""; GREEN=""; YELLOW=""; BLUE=""; RESET=""
fi

info()    { echo "${BLUE}${BOLD}==>${RESET} $*"; }
ok()      { echo "${GREEN}${BOLD} ✓${RESET} $*"; }
warn()    { echo "${YELLOW}${BOLD} ! ${RESET} $*" >&2; }
err()     { echo "${RED}${BOLD} ✗${RESET} $*" >&2; exit 1; }

# ── Argument parsing ──────────────────────────────────────────────────
while [ $# -gt 0 ]; do
  case "$1" in
    --version)     RIVEN_VERSION="$2"; shift 2 ;;
    --prefix)      RIVEN_HOME="$2"; shift 2 ;;
    --repo)        RIVEN_REPO="$2"; shift 2 ;;
    --from-source) FROM_SOURCE="${2:-.}"; shift 2 ;;
    --no-modify-path) NO_MODIFY_PATH=1; shift ;;
    -h|--help)
      cat <<EOF
Riven installer.

Usage: install.sh [options]

Options:
  --version <tag>       Release tag to install (default: latest)
  --prefix <dir>        Install root (default: \$HOME/.riven)
  --repo <owner/repo>   GitHub repo (default: sherazp995/riven)
  --from-source <path>  Build + install from a local riven checkout
                        instead of downloading a release. Pass "." for
                        the current directory. Requires \`cargo\` on PATH.
  --no-modify-path      Do not edit shell rc files
  -h, --help            Show this help

Examples:
  install.sh                              # install latest release
  install.sh --version v0.2.0             # install a pinned release
  install.sh --from-source .              # build + install from CWD
  install.sh --from-source ~/.projects/riven --prefix ~/.riven-dev
EOF
      exit 0
      ;;
    *) err "unknown flag: $1 (try --help)" ;;
  esac
done

# ── Tool checks ───────────────────────────────────────────────────────
need() { command -v "$1" >/dev/null 2>&1 || err "required tool missing: $1"; }
need uname
need mkdir
need tar
need mv
need chmod

if command -v curl >/dev/null 2>&1; then
  FETCH="curl -fsSL"
  FETCH_TO="curl -fsSL -o"
elif command -v wget >/dev/null 2>&1; then
  FETCH="wget -qO-"
  FETCH_TO="wget -qO"
else
  err "need curl or wget on PATH"
fi

# ── Detect platform ───────────────────────────────────────────────────
OS="$(uname -s)"
ARCH="$(uname -m)"

case "$OS" in
  Linux)  OS_TAG="unknown-linux-gnu" ;;
  Darwin) OS_TAG="apple-darwin" ;;
  *)      err "unsupported OS: $OS (only Linux and macOS are supported)" ;;
esac

case "$ARCH" in
  x86_64|amd64)   ARCH_TAG="x86_64" ;;
  aarch64|arm64)  ARCH_TAG="aarch64" ;;
  *)              err "unsupported architecture: $ARCH" ;;
esac

TARGET="${ARCH_TAG}-${OS_TAG}"
info "Detected platform: ${BOLD}${TARGET}${RESET}"

# ── Branch: local source build vs GitHub release ──────────────────────
# When --from-source is set we build the four binaries from a working
# tree and install them directly. Stdlib `.rvn` is embedded into the
# riven binary via `include_str!`, so no `library/` copy is needed —
# the resulting install at $RIVEN_HOME is fully self-contained.
if [ -n "$FROM_SOURCE" ]; then
  need cargo

  # Resolve to an absolute path so cd-by-the-user later doesn't break us.
  case "$FROM_SOURCE" in
    /*) ABS_SRC="$FROM_SOURCE" ;;
    *)  ABS_SRC="$(cd "$FROM_SOURCE" 2>/dev/null && pwd)" || \
        err "--from-source path not found: $FROM_SOURCE" ;;
  esac
  [ -f "$ABS_SRC/Cargo.toml" ] || \
    err "--from-source path is not a riven checkout (no Cargo.toml at $ABS_SRC)"
  [ -d "$ABS_SRC/compiler/riven_core" ] || \
    err "--from-source path doesn't look like riven (missing compiler/riven_core/)"

  # Tag the install with a local-build marker so `riven --version` and
  # the on-disk version file disambiguate from a release install.
  if command -v git >/dev/null 2>&1 && git -C "$ABS_SRC" rev-parse HEAD >/dev/null 2>&1; then
    TAG="local-$(git -C "$ABS_SRC" rev-parse --short HEAD)"
  else
    TAG="local"
  fi
  ok "Building Riven ${BOLD}${TAG}${RESET} from ${DIM}${ABS_SRC}${RESET}"

  # One build command, four binaries. Cargo bin targets carry the
  # underscore form (riven_lsp / riven_repl); the install loop below
  # checks both spellings when sourcing from $BIN_SRC_DIR so the on-
  # disk install ends up with the hyphenated release-tarball names.
  ( cd "$ABS_SRC" && \
    cargo build --release \
      --bin riven --bin rivenc --bin riven_lsp --bin riven_repl ) \
    || err "cargo build failed in $ABS_SRC"

  # Point the install loop at cargo's output dir.
  BIN_SRC_DIR="$ABS_SRC/target/release"
  EXTRA_SRC=""  # no lib/share/include payload — stdlib is embedded
else
  # ── Resolve release tag ─────────────────────────────────────────────
  if [ "$RIVEN_VERSION" = "latest" ]; then
  info "Resolving latest release..."
  API_URL="https://api.github.com/repos/${RIVEN_REPO}/releases/latest"
  TAG="$($FETCH "$API_URL" 2>/dev/null \
    | grep -o '"tag_name": *"[^"]*"' \
    | head -n1 \
    | sed 's/.*"tag_name": *"\([^"]*\)".*/\1/' || true)"
  if [ -z "$TAG" ]; then
    err "could not resolve latest release from ${RIVEN_REPO}. Pin one with --version <tag> or set RIVEN_VERSION."
  fi
else
  TAG="$RIVEN_VERSION"
fi
ok "Installing Riven ${BOLD}${TAG}${RESET}"

# ── Compute download URL ──────────────────────────────────────────────
ASSET="riven-${TAG}-${TARGET}.tar.gz"
URL="https://github.com/${RIVEN_REPO}/releases/download/${TAG}/${ASSET}"

# ── Download + extract ────────────────────────────────────────────────
TMP="$(mktemp -d "${TMPDIR:-/tmp}/riven-install.XXXXXX")"
trap 'rm -rf "$TMP"' EXIT

info "Downloading ${DIM}${URL}${RESET}"
if ! $FETCH_TO "$TMP/$ASSET" "$URL" 2>/dev/null; then
  err "download failed. Verify release assets exist at:
    https://github.com/${RIVEN_REPO}/releases/tag/${TAG}
  Expected asset name: ${ASSET}"
fi

info "Extracting..."
tar -xzf "$TMP/$ASSET" -C "$TMP"

# Accept either a flat tarball (bin/ at root) or nested (riven-*/bin/).
if [ -d "$TMP/bin" ]; then
  SRC="$TMP"
else
  SRC="$(find "$TMP" -maxdepth 2 -type d -name bin | head -n1 | xargs -I{} dirname {})"
  [ -n "$SRC" ] || err "archive does not contain a bin/ directory"
fi
  BIN_SRC_DIR="$SRC/bin"
  EXTRA_SRC="$SRC"
fi

# ── Install ───────────────────────────────────────────────────────────
mkdir -p "$RIVEN_HOME/bin"
info "Installing binaries to ${BOLD}${RIVEN_HOME}/bin${RESET}"
for bin in riven rivenc riven-lsp riven-repl; do
  # Accept either spelling at the source: release tarballs ship the
  # hyphen form (`riven-lsp`); cargo build emits the underscore form
  # (`riven_lsp`). The installed name is always the hyphen form so
  # `riven-lsp` works on PATH regardless of install path.
  src_hyphen="$BIN_SRC_DIR/$bin"
  src_underscore="$BIN_SRC_DIR/${bin//-/_}"
  if [ -f "$src_hyphen" ]; then
    src="$src_hyphen"
  elif [ -f "$src_underscore" ]; then
    src="$src_underscore"
  else
    warn "missing from build/archive: $bin"
    continue
  fi
  cp -f "$src" "$RIVEN_HOME/bin/$bin"
  chmod +x "$RIVEN_HOME/bin/$bin"
  ok "Installed ${BOLD}$bin${RESET}"
done

# Copy any supporting files (stdlib, runtime headers, etc.) — only
# applies to the release-tarball install path. Local builds embed
# stdlib into the binary so this loop is a no-op there.
if [ -n "$EXTRA_SRC" ]; then
  for dir in lib share include; do
    if [ -d "$EXTRA_SRC/$dir" ]; then
      mkdir -p "$RIVEN_HOME/$dir"
      cp -R "$EXTRA_SRC/$dir/." "$RIVEN_HOME/$dir/"
      ok "Installed ${BOLD}$dir${RESET}"
    fi
  done
fi

echo "$TAG" > "$RIVEN_HOME/version"

# ── Write env file ────────────────────────────────────────────────────
ENV_FILE="$RIVEN_HOME/env"
cat > "$ENV_FILE" <<'EOF'
# Riven toolchain environment.
# This file is sourced from your shell rc to put riven on PATH.

case ":${PATH}:" in
  *:"$HOME/.riven/bin":*) ;;
  *) export PATH="$HOME/.riven/bin:$PATH" ;;
esac
EOF
ok "Wrote ${BOLD}${ENV_FILE}${RESET}"

# ── Update shell rc files ─────────────────────────────────────────────
SOURCE_LINE='. "$HOME/.riven/env"'

update_rc() {
  local rc="$1"
  [ -f "$rc" ] || return 0
  if grep -Fq "$SOURCE_LINE" "$rc" 2>/dev/null; then
    return 0
  fi
  {
    printf '\n# Added by the Riven installer\n%s\n' "$SOURCE_LINE"
  } >> "$rc"
  ok "Updated ${BOLD}$rc${RESET}"
}

if [ "$NO_MODIFY_PATH" != "1" ]; then
  # Touch rc files that exist; don't create new ones.
  for rc in "$HOME/.bashrc" "$HOME/.bash_profile" "$HOME/.zshrc" "$HOME/.profile"; do
    update_rc "$rc"
  done
fi

# ── Final message ─────────────────────────────────────────────────────
echo
echo "${GREEN}${BOLD}Riven ${TAG} installed successfully.${RESET}"
echo
echo "To start using ${BOLD}riven${RESET} in the current shell, run:"
echo
echo "    ${BOLD}source \"\$HOME/.riven/env\"${RESET}"
echo
echo "Or open a new terminal. Then verify with:"
echo
echo "    ${BOLD}riven --version${RESET}"
echo "    ${BOLD}rivenc --version${RESET}"
echo
echo "Get started:  ${DIM}https://github.com/${RIVEN_REPO}/blob/master/docs/tutorial/01-getting-started.md${RESET}"
