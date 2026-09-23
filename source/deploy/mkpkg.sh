#!/usr/bin/env bash
# macOS .pkg from bundle; unsigned, see sign.sh
set -euo pipefail

BUNDLE="${1:?usage: mkpkg.sh <bundle-dir> [version] [out.pkg]}"
VERSION="${2:-0.1.0}"
OUT="${3:-j-$VERSION.pkg}"
IDENTIFIER="${J2_PKG_ID:-org.j2lang.j2}"
INSTALL_LOCATION="${J2_PKG_LOCATION:-/usr/local/j2}"

[ -x "$BUNDLE/bin/j2" ] || { echo "error: '$BUNDLE' is not a J2 bundle (no bin/j2)" >&2; exit 1; }
command -v pkgbuild >/dev/null || { echo "error: pkgbuild not found (Xcode command line tools)" >&2; exit 1; }

echo "Packaging J2 $VERSION from $BUNDLE -> $OUT"

SCRATCH="$(mktemp -d)"
trap 'rm -rf "$SCRATCH"' EXIT
SCRIPTS="$SCRATCH/scripts"
mkdir -p "$SCRIPTS"

# postinstall links j, warms cache ($2 location)
cat > "$SCRIPTS/postinstall" <<POST
#!/bin/bash
set -e
PREFIX="\${2%/}/${INSTALL_LOCATION#/}"
# absolute install-location puts payload there directly
[ -x "\$PREFIX/bin/j2" ] || PREFIX="${INSTALL_LOCATION}"
mkdir -p /usr/local/bin
ln -sf "\$PREFIX/bin/j2" /usr/local/bin/j2 || true
TMP="\$(mktemp -d)"; printf 'print("ok")\n' > "\$TMP/warm.j2"
"\$PREFIX/bin/j2" build "\$TMP/warm.j2" -o "\$TMP/warm.out" >/dev/null 2>&1 || true
rm -rf "\$TMP"
exit 0
POST
chmod +x "$SCRIPTS/postinstall"

pkgbuild \
  --root "$BUNDLE" \
  --identifier "$IDENTIFIER" \
  --version "$VERSION" \
  --install-location "$INSTALL_LOCATION" \
  --scripts "$SCRIPTS" \
  "$OUT"

echo
echo "Built $OUT ($(du -h "$OUT" | cut -f1))."
echo "  local install:  sudo installer -pkg $OUT -target /"
echo "  distribute:     sign + notarize first (deploy/sign.sh)."
