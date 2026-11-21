# Distributing J2 on macOS

J2 v2 is version `0.2.0`. The default downloads come from the
[latest release](https://github.com/JasnamSinghArora/j2/releases/latest).
Previous releases remain available in the release history.

J2 ships as a self-contained bundle. The `j2` binary finds its toolchain,
vendored crates, and runtime relative to itself, so native builds work without
a separately installed compiler or a network connection.

## Bundle layout

```text
<bundle>/
  bin/j2                 driver and interpreter
  toolchain/bin/         compiler and cargo
  toolchain/lib/         standard library and compiler libraries
  j2-vendor/             dependencies for offline builds
  library/j2_runtime/    runtime source
  install.sh
  VERSION
```

## Release flow

Use the full [compiler checkout](https://github.com/JasnamSinghArora/ProgrammingLanguage-Common)
to build a release. The public source export contains the J2 frontend, driver,
and runtime, but does not duplicate the compiler backend build tree.

1. Build the backend with `./x.py build --stage 1`, then build the driver:
   `cargo build --release --manifest-path src/bin/j2/Cargo.toml --offline`.
   Make sure `j2-vendor/` contains the dependencies in `program.lock`.
2. Set `DEVELOPER_ID` to the Developer ID Application identity and
   `NOTARY_PROFILE` to the existing notarytool Keychain profile. Run
   `deploy/mkrelease.sh`. It reads the version from the driver manifest, checks
   the built driver, signs the bundle, notarizes the ZIP, and creates the
   tarball from the signed bundle.
3. Build an installer with
   `deploy/mkpkg.sh dist/j2-0.2.0-aarch64-apple-darwin 0.2.0 dist/j2-0.2.0-unsigned.pkg`.
   Use `productsign --sign "$INSTALLER_ID"` to write the final
   `dist/j2-0.2.0.pkg`, where `INSTALLER_ID` is a Developer ID Installer identity.
   Submit that file using `xcrun notarytool submit` with the Keychain profile
   and `--wait`, then run `xcrun stapler staple dist/j2-0.2.0.pkg` after acceptance.
4. Package the VS Code extension at the same version. Test the extracted
   archive with both the interpreter and native execution, verify signatures,
   and calculate SHA-256 checksums after all signing and stapling is complete.
5. Publish the tag and release assets. Update the website and Homebrew formula
   with the final archive version and checksum, and mark the release as latest.

Do not rerun `mkbundle.sh` over a signed bundle: it replaces the directory.
`mkrelease.sh` signs before archiving, so its archives contain the final code
signatures. Without `DEVELOPER_ID`, it produces unsigned archives for local use.

## Artifacts

- `j2-0.2.0-aarch64-apple-darwin.tar.gz`: the bundle and installer script.
- `j2-0.2.0-aarch64-apple-darwin.zip`: the notarized bundle.
- `j2-0.2.0.pkg`: the signed, notarized installer with a stapled ticket.
- `j2-lang-0.2.0.vsix`: the VS Code extension.
- `SHA256SUMS`: checksums of the published files.

## Install

For the tarball, extract it and run `./install.sh` inside the extracted
`j2-0.2.0-aarch64-apple-darwin` directory. It installs to `~/.j2` and links `j2`
into `/usr/local/bin`. Use `J2_PREFIX` and `J2_BINDIR` to choose other locations.
The older `j` alias is kept when it does not replace another program.

The `.pkg` installer uses `/usr/local/j2` and `/usr/local/bin/j2`.
The Homebrew formula is `deploy/Formula/j2.rb` in the source tree and
`Formula/j2.rb` at the root of the public repository.

## Signing details

The compiler loads proc macros built on the user's machine. Its hardened
runtime signature needs `com.apple.security.cs.disable-library-validation`,
which `sign.sh` applies to `toolchain/bin/rustc`. Removing it prevents native
builds that use those dependencies.

A Developer ID Application identity signs the executables and libraries.
A separate Developer ID Installer identity signs the `.pkg` container.
Notarize both the ZIP and installer, and staple the installer after Apple
accepts it. A plain directory or ZIP cannot hold a stapled ticket.

## Build cache

Cargo caches contain absolute paths, so the release does not include a built
cache. `install.sh` warms it at the final location. The interpreter is ready
immediately; the cache only speeds up native builds.
