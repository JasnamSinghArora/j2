#!/usr/bin/env bash
# sign/notarize bundle for Gatekeeper; needs Developer ID
set -euo pipefail
BUNDLE="${1:?usage: sign.sh <bundle-dir>}"
: "${DEVELOPER_ID:?set DEVELOPER_ID to your 'Developer ID Application' identity}"

echo "Signing every Mach-O in $BUNDLE (hardened runtime, timestamped)..."
# Sign dylibs + executables individually first
find "$BUNDLE" -type f \( -name '*.dylib' -o -name '*.so' \) -print0 |
  while IFS= read -r -d '' f; do
    codesign --force --options runtime --timestamp --sign "$DEVELOPER_ID" "$f"
  done
find "$BUNDLE" -type f -perm +111 ! -name '*.dylib' -print0 |
  while IFS= read -r -d '' f; do
    codesign --force --options runtime --timestamp --sign "$DEVELOPER_ID" "$f" 2>/dev/null || true
  done
codesign --force --options runtime --timestamp --sign "$DEVELOPER_ID" "$BUNDLE/bin/j2"
codesign --verify --deep --strict "$BUNDLE/bin/j2" && echo "signature verified"

# library-validation exemption so user-built proc-macro dylibs load
ENT="$(mktemp -t rustc-entitlements).plist"
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
if [ -x "$BUNDLE/toolchain/bin/rustc" ]; then
  codesign --force --options runtime --timestamp --entitlements "$ENT" \
    --sign "$DEVELOPER_ID" "$BUNDLE/toolchain/bin/rustc"
  echo "compiler re-signed with library-validation exemption"
fi
rm -f "$ENT"

if [ -n "${NOTARY_PROFILE:-}" ]; then
  echo "Notarizing..."
  ZIP="$BUNDLE.zip"
  ditto -c -k --keepParent "$BUNDLE" "$ZIP"
  xcrun notarytool submit "$ZIP" --keychain-profile "$NOTARY_PROFILE" --wait
  # Stapling a directory bundle isn't supported
  echo "Notarized $ZIP. (For a stapled artifact, wrap in a .pkg/.dmg and staple that.)"
else
  echo "Set NOTARY_PROFILE to also notarize. Without notarization, Gatekeeper"
  echo "will warn on first launch of downloaded binaries."
fi
