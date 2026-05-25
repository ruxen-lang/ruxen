#!/usr/bin/env bash
#
# Ruxen installer.
#
# Installs the Ruxen toolchain (ruxen, ruxenc, ruxen-lsp, ruxen-repl)
# from GitHub Releases into ~/.ruxen and configures PATH.
#
# Usage:
#   curl -fsSL https://raw.githubusercontent.com/ruxen-lang/ruxen/master/install.sh | bash
#   curl -fsSL https://raw.githubusercontent.com/ruxen-lang/ruxen/master/install.sh | bash -s -- --version v0.1.0
#
# Environment overrides:
#   RUXEN_VERSION   Pin a specific release tag (default: latest)
#   RUXEN_REPO      owner/repo on GitHub (default: ruxen-lang/ruxen)
#   RUXEN_HOME      Install root (default: $HOME/.ruxen)
#   RUXEN_NO_MODIFY_PATH=1    Skip editing shell rc files

set -euo pipefail

RUXEN_REPO="${RUXEN_REPO:-ruxen-lang/ruxen}"
RUXEN_HOME="${RUXEN_HOME:-$HOME/.ruxen}"
RUXEN_VERSION="${RUXEN_VERSION:-latest}"
NO_MODIFY_PATH="${RUXEN_NO_MODIFY_PATH:-0}"
# Set to a ruxen source checkout path to skip GitHub releases and
# build + install from that working tree instead. Cargo on PATH is
# required when this is non-empty.
FROM_SOURCE="${RUXEN_FROM_SOURCE:-}"

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
    --version)     RUXEN_VERSION="$2"; shift 2 ;;
    --prefix)      RUXEN_HOME="$2"; shift 2 ;;
    --repo)        RUXEN_REPO="$2"; shift 2 ;;
    --from-source) FROM_SOURCE="${2:-.}"; shift 2 ;;
    --no-modify-path) NO_MODIFY_PATH=1; shift ;;
    -h|--help)
      cat <<EOF
Ruxen installer.

Usage: install.sh [options]

Options:
  --version <tag>       Release tag to install (default: latest)
  --prefix <dir>        Install root (default: \$HOME/.ruxen)
  --repo <owner/repo>   GitHub repo (default: ruxen-lang/ruxen)
  --from-source <path>  Build + install from a local ruxen checkout
                        instead of downloading a release. Pass "." for
                        the current directory. Requires \`cargo\` on PATH.
  --no-modify-path      Do not edit shell rc files
  -h, --help            Show this help

Examples:
  install.sh                              # install latest release
  install.sh --version v0.2.0             # install a pinned release
  install.sh --from-source .              # build + install from CWD
  install.sh --from-source ~/.projects/ruxen --prefix ~/.ruxen-dev
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
# tree and install them directly. Stdlib `.rx` is embedded into the
# ruxen binary via `include_str!`, so no `library/` copy is needed —
# the resulting install at $RUXEN_HOME is fully self-contained.
if [ -n "$FROM_SOURCE" ]; then
  need cargo

  # Resolve to an absolute path so cd-by-the-user later doesn't break us.
  case "$FROM_SOURCE" in
    /*) ABS_SRC="$FROM_SOURCE" ;;
    *)  ABS_SRC="$(cd "$FROM_SOURCE" 2>/dev/null && pwd)" || \
        err "--from-source path not found: $FROM_SOURCE" ;;
  esac
  [ -f "$ABS_SRC/Cargo.toml" ] || \
    err "--from-source path is not a ruxen checkout (no Cargo.toml at $ABS_SRC)"
  [ -d "$ABS_SRC/compiler/ruxen_core" ] || \
    err "--from-source path doesn't look like ruxen (missing compiler/ruxen_core/)"

  # Tag the install with a local-build marker so `ruxen --version` and
  # the on-disk version file disambiguate from a release install.
  if command -v git >/dev/null 2>&1 && git -C "$ABS_SRC" rev-parse HEAD >/dev/null 2>&1; then
    TAG="local-$(git -C "$ABS_SRC" rev-parse --short HEAD)"
  else
    TAG="local"
  fi
  ok "Building Ruxen ${BOLD}${TAG}${RESET} from ${DIM}${ABS_SRC}${RESET}"

  # One build command, four binaries. Cargo bin targets carry the
  # underscore form (ruxen_lsp / ruxen_repl); the install loop below
  # checks both spellings when sourcing from $BIN_SRC_DIR so the on-
  # disk install ends up with the hyphenated release-tarball names.
  ( cd "$ABS_SRC" && \
    cargo build --release \
      --bin ruxen --bin ruxenc --bin ruxen_lsp --bin ruxen_repl ) \
    || err "cargo build failed in $ABS_SRC"

  # Point the install loop at cargo's output dir.
  BIN_SRC_DIR="$ABS_SRC/target/release"
  EXTRA_SRC=""  # no lib/share/include payload — stdlib is embedded
else
  # ── Resolve release tag ─────────────────────────────────────────────
  if [ "$RUXEN_VERSION" = "latest" ]; then
  info "Resolving latest release..."
  API_URL="https://api.github.com/repos/${RUXEN_REPO}/releases/latest"
  TAG="$($FETCH "$API_URL" 2>/dev/null \
    | grep -o '"tag_name": *"[^"]*"' \
    | head -n1 \
    | sed 's/.*"tag_name": *"\([^"]*\)".*/\1/' || true)"
  if [ -z "$TAG" ]; then
    err "could not resolve latest release from ${RUXEN_REPO}. Pin one with --version <tag> or set RUXEN_VERSION."
  fi
else
  TAG="$RUXEN_VERSION"
fi
ok "Installing Ruxen ${BOLD}${TAG}${RESET}"

# ── Compute download URL ──────────────────────────────────────────────
ASSET="ruxen-${TAG}-${TARGET}.tar.gz"
URL="https://github.com/${RUXEN_REPO}/releases/download/${TAG}/${ASSET}"

# ── Download + extract ────────────────────────────────────────────────
TMP="$(mktemp -d "${TMPDIR:-/tmp}/ruxen-install.XXXXXX")"
trap 'rm -rf "$TMP"' EXIT

info "Downloading ${DIM}${URL}${RESET}"
if ! $FETCH_TO "$TMP/$ASSET" "$URL" 2>/dev/null; then
  err "download failed. Verify release assets exist at:
    https://github.com/${RUXEN_REPO}/releases/tag/${TAG}
  Expected asset name: ${ASSET}"
fi

info "Extracting..."
tar -xzf "$TMP/$ASSET" -C "$TMP"

# Accept either a flat tarball (bin/ at root) or nested (ruxen-*/bin/).
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
mkdir -p "$RUXEN_HOME/bin"
info "Installing binaries to ${BOLD}${RUXEN_HOME}/bin${RESET}"
for bin in ruxen ruxenc ruxen-lsp ruxen-repl; do
  # Accept either spelling at the source: release tarballs ship the
  # hyphen form (`ruxen-lsp`); cargo build emits the underscore form
  # (`ruxen_lsp`). The installed name is always the hyphen form so
  # `ruxen-lsp` works on PATH regardless of install path.
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
  cp -f "$src" "$RUXEN_HOME/bin/$bin"
  chmod +x "$RUXEN_HOME/bin/$bin"
  ok "Installed ${BOLD}$bin${RESET}"
done

# Copy any supporting files (stdlib, runtime headers, etc.) — only
# applies to the release-tarball install path. Local builds embed
# stdlib into the binary so this loop is a no-op there.
if [ -n "$EXTRA_SRC" ]; then
  for dir in lib share include; do
    if [ -d "$EXTRA_SRC/$dir" ]; then
      mkdir -p "$RUXEN_HOME/$dir"
      cp -R "$EXTRA_SRC/$dir/." "$RUXEN_HOME/$dir/"
      ok "Installed ${BOLD}$dir${RESET}"
    fi
  done
fi

echo "$TAG" > "$RUXEN_HOME/version"

# ── Write env file ────────────────────────────────────────────────────
ENV_FILE="$RUXEN_HOME/env"
cat > "$ENV_FILE" <<'EOF'
# Ruxen toolchain environment.
# This file is sourced from your shell rc to put ruxen on PATH.

case ":${PATH}:" in
  *:"$HOME/.ruxen/bin":*) ;;
  *) export PATH="$HOME/.ruxen/bin:$PATH" ;;
esac
EOF
ok "Wrote ${BOLD}${ENV_FILE}${RESET}"

# ── Update shell rc files ─────────────────────────────────────────────
SOURCE_LINE='. "$HOME/.ruxen/env"'

update_rc() {
  local rc="$1"
  [ -f "$rc" ] || return 0
  if grep -Fq "$SOURCE_LINE" "$rc" 2>/dev/null; then
    return 0
  fi
  {
    printf '\n# Added by the Ruxen installer\n%s\n' "$SOURCE_LINE"
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
echo "${GREEN}${BOLD}Ruxen ${TAG} installed successfully.${RESET}"
echo
echo "To start using ${BOLD}ruxen${RESET} in the current shell, run:"
echo
echo "    ${BOLD}source \"\$HOME/.ruxen/env\"${RESET}"
echo
echo "Or open a new terminal. Then verify with:"
echo
echo "    ${BOLD}ruxen --version${RESET}"
echo "    ${BOLD}ruxenc --version${RESET}"
echo
echo "Get started:  ${DIM}https://github.com/${RUXEN_REPO}/blob/master/docs/tutorial/01-getting-started.md${RESET}"
