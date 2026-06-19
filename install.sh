#!/bin/sh
# Abe installer — downloads a prebuilt binary from GitHub Releases and writes a
# starter config. Usage:
#   curl -fsSL https://raw.githubusercontent.com/yonk-labs/abe/main/install.sh | sh
#
# Overrides (env): ABE_INSTALL_DIR, ABE_VERSION (default: latest).
# No Rust toolchain needed. Linux x86_64 and macOS (arm64/x86_64) only — for
# other platforms use:  cargo install --git https://github.com/yonk-labs/abe
set -eu

REPO="yonk-labs/abe"
BIN="abe"
INSTALL_DIR="${ABE_INSTALL_DIR:-$HOME/.local/bin}"
CONFIG_DIR="${XDG_CONFIG_HOME:-$HOME/.config}/abe"
VERSION="${ABE_VERSION:-latest}"

err() { echo "abe-install: $*" >&2; exit 1; }

# --- detect platform ---
os=$(uname -s)
arch=$(uname -m)
case "$os" in
  Linux)  os_tag=linux ;;
  Darwin) os_tag=macos ;;
  *) err "unsupported OS '$os'. Try: cargo install --git https://github.com/$REPO" ;;
esac
case "$arch" in
  x86_64|amd64)  arch_tag=x86_64 ;;
  arm64|aarch64) arch_tag=arm64 ;;
  *) err "unsupported arch '$arch'. Try: cargo install --git https://github.com/$REPO" ;;
esac
if [ "$os_tag" = linux ] && [ "$arch_tag" = arm64 ]; then
  err "no prebuilt linux/arm64 binary. Try: cargo install --git https://github.com/$REPO"
fi

asset="${BIN}-${os_tag}-${arch_tag}"
if [ "$VERSION" = latest ]; then
  url="https://github.com/$REPO/releases/latest/download/$asset"
else
  url="https://github.com/$REPO/releases/download/$VERSION/$asset"
fi

# --- pick a downloader ---
if command -v curl >/dev/null 2>&1; then
  download() { curl -fsSL -o "$1" "$2"; }
elif command -v wget >/dev/null 2>&1; then
  download() { wget -qO "$1" "$2"; }
else
  err "need curl or wget on PATH"
fi

# --- download + install ---
mkdir -p "$INSTALL_DIR"
tmp=$(mktemp)
trap 'rm -f "$tmp"' EXIT INT TERM
echo "abe-install: downloading $asset ($VERSION)..."
download "$tmp" "$url" || err "download failed from $url"
chmod +x "$tmp"
mv "$tmp" "$INSTALL_DIR/$BIN"
trap - EXIT INT TERM
echo "abe-install: installed -> $INSTALL_DIR/$BIN"

# --- configure models via the interactive wizard ---
# `curl | sh` can't read stdin (it IS the script), so run `abe init` against the
# terminal through /dev/tty when one is available. Never clobber an existing config.
if [ -f "$CONFIG_DIR/config.yaml" ]; then
  echo "abe-install: existing config at $CONFIG_DIR/config.yaml (left as-is; run 'abe init' to redo)"
elif [ -r /dev/tty ]; then
  echo "abe-install: launching setup wizard (abe init)..."
  "$INSTALL_DIR/$BIN" init < /dev/tty || echo "abe-install: setup skipped — run 'abe init' anytime"
else
  echo "abe-install: no terminal detected — run 'abe init' to set up your models"
fi

# --- PATH hint ---
case ":$PATH:" in
  *":$INSTALL_DIR:"*) ;;
  *) echo "abe-install: NOTE: $INSTALL_DIR is not on your PATH. Add:"
     echo "    export PATH=\"$INSTALL_DIR:\$PATH\"" ;;
esac

echo "abe-install: done. Try:  $BIN models"
