#!/usr/bin/env bash
# Sign a macOS J2 bundle, then notarize its ZIP if NOTARY_PROFILE is set.
# DEVELOPER_ID must name a Developer ID Application identity in Keychain.
# Usage: deploy/sign.sh <bundle-dir>
set -euo pipefail
BUNDLE="${1:?usage: sign.sh <bundle-dir>}"
: "${DEVELOPER_ID:?set DEVELOPER_ID to your 'Developer ID Application' identity}"
[ -x "$BUNDLE/bin/j2" ] || { echo "error: no J2 driver in $BUNDLE" >&2; exit 1; }
SCRATCH="$(mktemp -d)"
trap 'rm -rf "$SCRATCH"' EXIT
ENT="$SCRATCH/rustc-entitlements.plist"
# rustc loads proc macros built on the user's machine with ad-hoc signatures.
# Without this entitlement, hardened runtime prevents those native builds.
cat > "$ENT" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>com.apple.security.cs.disable-library-validation</key>
    <true/>
</dict>
</plist>
PLIST

echo "Signing Mach-O libraries and executables in $BUNDLE"
find "$BUNDLE" -type f \( -name '*.dylib' -o -name '*.so' -o -perm +111 \) -print0 |
  while IFS= read -r -d '' f; do
    # Shell scripts and source files are not Mach-O code and need no signature.
    if ! file -b "$f" | grep -q 'Mach-O'; then
      continue
    fi
    if [ "$f" = "$BUNDLE/toolchain/bin/rustc" ]; then
      codesign --force --options runtime --timestamp --entitlements "$ENT" --sign "$DEVELOPER_ID" "$f"
    else
      codesign --force --options runtime --timestamp --sign "$DEVELOPER_ID" "$f"
    fi
    codesign --verify --strict "$f"
  done
codesign --verify --deep --strict "$BUNDLE/bin/j2"

if [ -n "${NOTARY_PROFILE:-}" ]; then
  ZIP="$BUNDLE.zip"
  ditto -c -k --keepParent "$BUNDLE" "$ZIP"
  echo "Submitting $ZIP for notarization"
  xcrun notarytool submit "$ZIP" --keychain-profile "$NOTARY_PROFILE" --wait --output-format json > "$SCRATCH/notary.json"
  cat "$SCRATCH/notary.json"
  STATUS="$(plutil -extract status raw -o - "$SCRATCH/notary.json")"
  [ "$STATUS" = "Accepted" ] || { echo "error: notarization status is $STATUS" >&2; exit 1; }
  echo "Notarized $ZIP. For a stapled download, sign and notarize a .pkg installer."
else
  echo "Bundle signed. Set NOTARY_PROFILE to also notarize its ZIP."
fi
