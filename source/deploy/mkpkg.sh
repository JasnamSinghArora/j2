#!/usr/bin/env bash
# Build a macOS installer from a J2 bundle.
# Usage: deploy/mkpkg.sh <bundle-dir> [version] [out.pkg]
# The package installs to /usr/local/j2 and links j2 onto PATH.
# Sign the result with productsign, then notarize and staple it before release.
set -euo pipefail

BUNDLE="${1:?usage: mkpkg.sh <bundle-dir> [version] [out.pkg]}"
[ -x "$BUNDLE/bin/j2" ] || { echo "error: '$BUNDLE' is not a J2 bundle (no bin/j2)" >&2; exit 1; }
BUNDLE_VERSION="$("$BUNDLE/bin/j2" --version | sed 's/^j2 //')"
VERSION="${2:-$BUNDLE_VERSION}"
[ "$VERSION" = "$BUNDLE_VERSION" ] || {
  echo "error: package version $VERSION does not match bundled driver $BUNDLE_VERSION" >&2
  exit 1
}
OUT="${3:-j2-$VERSION.pkg}"
IDENTIFIER="${J2_PKG_ID:-org.j2lang.j2}"
INSTALL_LOCATION="${J2_PKG_LOCATION:-/usr/local/j2}"

command -v pkgbuild >/dev/null || { echo "error: pkgbuild not found (Xcode command line tools)" >&2; exit 1; }

echo "Packaging J2 $VERSION from $BUNDLE -> $OUT"

SCRATCH="$(mktemp -d)"
trap 'rm -rf "$SCRATCH"' EXIT
SCRIPTS="$SCRATCH/scripts"
mkdir -p "$SCRIPTS"

# Installer passes the target volume as $3.
cat > "$SCRIPTS/postinstall" <<POST
#!/bin/bash
set -e
VOLUME="\${3:-/}"
PREFIX="\${VOLUME%/}/${INSTALL_LOCATION#/}"
BINDIR="\${VOLUME%/}/usr/local/bin"
mkdir -p "\$BINDIR"
ln -sf "${INSTALL_LOCATION}/bin/j2" "\$BINDIR/j2"
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
echo "  distribute:     productsign, notarize with notarytool, then staple the installer."
