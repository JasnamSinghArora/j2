#!/usr/bin/env bash
# Build a distributable J2 release tarball
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
VER="${1:-$(date +%Y.%m.%d)}"
OUT="${2:-$ROOT/dist}"
TRIPLE="aarch64-apple-darwin"
NAME="j2-$VER-$TRIPLE"
BUNDLE="$OUT/$NAME"

mkdir -p "$OUT"
bash "$ROOT/deploy/mkbundle.sh" "$BUNDLE"
cp "$ROOT/deploy/install.sh" "$BUNDLE/install.sh"; chmod +x "$BUNDLE/install.sh"
printf '%s\n' "$VER" > "$BUNDLE/VERSION"

echo "Packaging $NAME.tar.gz ..."
( cd "$OUT" && tar -czf "$NAME.tar.gz" "$NAME" )
SHA=$(shasum -a 256 "$OUT/$NAME.tar.gz" | cut -d' ' -f1)
echo "Release:  $OUT/$NAME.tar.gz  ($(du -sh "$OUT/$NAME.tar.gz" | cut -f1))"
echo "sha256:   $SHA"
echo
echo "Next: deploy/sign.sh \"$BUNDLE\" (optional, needs Apple Developer ID), then"
echo "      upload the tarball + fill url/sha256 into deploy/Formula/j.rb."
