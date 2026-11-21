#!/usr/bin/env bash
# Install an extracted J2 bundle. Override paths with J2_PREFIX and J2_BINDIR.
set -euo pipefail
SRC="$(cd "$(dirname "$0")" && pwd)"
PREFIX="${J2_PREFIX:-$HOME/.j2}"
BINDIR="${J2_BINDIR:-/usr/local/bin}"

[ -x "$SRC/bin/j2" ] || { echo "error: run this from inside an extracted J2 bundle" >&2; exit 1; }
# Do not remove the source bundle when installing from its existing location.
if [ -d "$PREFIX" ] && [ "$(cd "$PREFIX" && pwd)" = "$SRC" ]; then
  echo "J2 is already installed at $PREFIX"
else
  echo "Installing J2 -> $PREFIX"
  rm -rf "$PREFIX"
  mkdir -p "$PREFIX"
  cp -R "$SRC/bin" "$SRC/toolchain" "$SRC/j2-vendor" "$SRC/library" "$PREFIX/"
  for metadata in VERSION LICENSE-MIT LICENSE-APACHE THIRD-PARTY-NOTICES.md; do
    [ ! -f "$SRC/$metadata" ] || cp "$SRC/$metadata" "$PREFIX/"
  done
fi

if mkdir -p "$BINDIR" 2>/dev/null && ln -sf "$PREFIX/bin/j2" "$BINDIR/j2" 2>/dev/null; then
  echo "Linked $BINDIR/j2 -> $PREFIX/bin/j2"
  # Preserve the older short command without replacing another program.
  if [ ! -e "$BINDIR/j" ] || [ "$(readlink "$BINDIR/j" 2>/dev/null || true)" = "$PREFIX/bin/j2" ]; then
    ln -sf "$PREFIX/bin/j2" "$BINDIR/j" 2>/dev/null || true
  fi
else
  case "${SHELL##*/}" in
    zsh) RC="$HOME/.zshrc" ;;
    bash) RC="$HOME/.bashrc" ;;
    *) RC="$HOME/.profile" ;;
  esac
  if grep -qsF "$PREFIX/bin" "$RC"; then
    echo "PATH entry for $PREFIX/bin already present in $RC"
  else
    printf '\n# Added by the J2 installer\nexport PATH="%s/bin:$PATH"\n' "$PREFIX" >> "$RC"
    echo "Added $PREFIX/bin to PATH in $RC"
  fi
  echo "Open a new terminal (or run: source $RC) and j2 will be available."
fi

echo "Warming the native build cache. The interpreter is ready to use."
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT
printf 'print("ok")\n' > "$TMP/warm.j2"
"$PREFIX/bin/j2" build "$TMP/warm.j2" -o "$TMP/warm.out" >/dev/null 2>&1 || true

echo "Done. Run j2 --version to check your installation."
