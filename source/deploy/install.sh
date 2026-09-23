#!/usr/bin/env bash
# J2 macOS installer; run after extracting bundle
set -euo pipefail
SRC="$(cd "$(dirname "$0")" && pwd)"
PREFIX="${J2_PREFIX:-$HOME/.j2}"
BINDIR="${J2_BINDIR:-/usr/local/bin}"

[ -x "$SRC/bin/j2" ] || { echo "error: run this from inside an extracted J2 bundle" >&2; exit 1; }

echo "Installing J2 -> $PREFIX"
rm -rf "$PREFIX"; mkdir -p "$PREFIX"
cp -R "$SRC/bin" "$SRC/toolchain" "$SRC/j2-vendor" "$SRC/library" "$PREFIX/"

# Put `j` on PATH.
if mkdir -p "$BINDIR" 2>/dev/null && ln -sf "$PREFIX/bin/j2" "$BINDIR/j" 2>/dev/null; then
  echo "Linked $BINDIR/j -> $PREFIX/bin/j2"
else
  # No write access to $BINDIR
  case "${SHELL##*/}" in
    zsh)  RC="$HOME/.zshrc" ;;
    bash) RC="$HOME/.bashrc" ;;
    *)    RC="$HOME/.profile" ;;
  esac
  if grep -qsF "$PREFIX/bin" "$RC"; then
    echo "PATH entry for $PREFIX/bin already present in $RC"
  else
    printf '\n# Added by the J2 installer\nexport PATH="%s/bin:$PATH"\n' "$PREFIX" >> "$RC"
    echo "Added $PREFIX/bin to PATH in $RC"
  fi
  echo "Open a new terminal (or run:  source $RC) and \`j\` will be available."
fi

# warm native cache; target dirs not relocatable
echo "Warming the native build cache (one-time, safe to Ctrl-C, the interpreter needs no cache)..."
TMP="$(mktemp -d)"; printf 'print("ok")\n' > "$TMP/warm.j2"
"$PREFIX/bin/j2" build "$TMP/warm.j2" -o "$TMP/warm.out" >/dev/null 2>&1 || true
rm -rf "$TMP"

echo
echo "Done.  Try:  j2 --version 2>/dev/null; echo 'print(\"hello\")' > h.j2 && j h.j2"
