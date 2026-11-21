#!/usr/bin/env bash
# Assemble a J2 release. If DEVELOPER_ID is set, sign before making archives.
# Usage: deploy/mkrelease.sh [version] [out-dir]
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
SOURCE_VERSION="$(sed -n 's/^version = "\([^"]*\)"/\1/p' "$ROOT/src/bin/j2/Cargo.toml" | head -n 1)"
VER="${1:-$SOURCE_VERSION}"
OUT="${2:-$ROOT/dist}"
TRIPLE="aarch64-apple-darwin"
NAME="j2-$VER-$TRIPLE"
BUNDLE="$OUT/$NAME"

[ -n "$VER" ] && [ "$VER" = "$SOURCE_VERSION" ] || {
  echo "error: release version must match the driver manifest ($SOURCE_VERSION)" >&2
  exit 1
}
mkdir -p "$OUT"
bash "$ROOT/deploy/mkbundle.sh" "$BUNDLE"
if [ -n "${DEVELOPER_ID:-}" ]; then
  bash "$ROOT/deploy/sign.sh" "$BUNDLE"
else
  echo "Building unsigned archives for local use. Set DEVELOPER_ID and NOTARY_PROFILE for a public release."
fi

# Package the final signed bytes. Never rebuild the bundle after signing it.
echo "Packaging $NAME"
( cd "$OUT" && tar -czf "$NAME.tar.gz" "$NAME" )
if [ -z "${NOTARY_PROFILE:-}" ] || [ -z "${DEVELOPER_ID:-}" ]; then
  ditto -c -k --keepParent "$BUNDLE" "$BUNDLE.zip"
fi
shasum -a 256 "$OUT/$NAME.tar.gz" "$OUT/$NAME.zip"
echo "Release archives: $OUT"
echo "Build the installer with deploy/mkpkg.sh, then sign, notarize, and staple it."
echo "Update the Homebrew formula and website checksums before publishing."
