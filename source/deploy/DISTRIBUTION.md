# Distributing J2 on macOS

J2 ships as one self-contained bundle. The `j2` binary finds its toolchain,
vendored crates, and runtime relative to itself, so the bundle can live anywhere
and needs no system toolchain, cargo, or network.

## Bundle layout

```
<bundle>/
  bin/j2                 driver (interpreter first, native via `j2 build`)
  toolchain/bin/         pruned compiler and cargo
  toolchain/lib/         std sysroot
  j2-vendor/             183 vendored crates for offline builds
  library/j2_runtime/    runtime source
  install.sh
  VERSION
```

About 670 MB unpacked: toolchain 400 MB, vendor 265 MB.

## Release flow

1. Build: `x.py build --stage 1`, then `(cd src/bin/j2 && cargo build --release)`.
   `j2-vendor/` must exist; re-vendor from `program.lock` if deps changed.
2. Package: `deploy/mkrelease.sh [version]` writes
   `dist/j2-<ver>-aarch64-apple-darwin.tar.gz` and prints its sha256.
3. Sign and notarize: `deploy/sign.sh <bundle>`. Then re-tar by hand, since
   `mkrelease.sh` rebuilds the bundle and the step 2 tarball holds unsigned bytes:
   `(cd dist && tar -czf j2-<ver>-aarch64-apple-darwin.tar.gz j2-<ver>-aarch64-apple-darwin)`.
4. Publish: upload the tarball and fill `url`, `sha256`, and `version` in
   `deploy/Formula/j2.rb`.

## Artifacts

- `j2-<ver>-aarch64-apple-darwin.tar.gz`, the bundle plus `install.sh`.
- `j2-<ver>-aarch64-apple-darwin.zip`, the same bytes as notarized.
- `j2-<ver>.pkg`, installs to `/usr/local/j2`. The payload is signed and
  notarized; the container needs a Developer ID Installer certificate.
- `j2-lang-<ver>.vsix`, the VS Code extension.

## Install

- Tarball: `tar xzf j2-<ver>-aarch64-apple-darwin.tar.gz`, `cd` into it, run
  `./install.sh`. Installs to `~/.j2` and links `j2` into `/usr/local/bin`.
  Override with `J2_PREFIX` and `J2_BINDIR`.
- Homebrew: `brew install --formula deploy/Formula/j2.rb`.

## Code signing

Gatekeeper blocks downloaded binaries unless they are signed with a Developer
ID Application certificate and notarized. This needs an Apple Developer account:

1. Create a Developer ID Application certificate in Keychain.
2. Store a notarytool profile: `xcrun notarytool store-credentials <name>`.
3. `DEVELOPER_ID="Developer ID Application: NAME (TEAMID)" NOTARY_PROFILE=<name> deploy/sign.sh <bundle>`.

To staple, wrap the bundle in a `.pkg` or `.dmg` and staple that. Unsigned
bundles still run after `xattr -dr com.apple.quarantine <bundle>`.

## Build cache

The cargo target cache holds absolute paths, so it is not prebuilt into the
bundle. `install.sh` warms it once at the final location. The interpreter never
needs it; the warm-up only speeds the first `j2 build`.
