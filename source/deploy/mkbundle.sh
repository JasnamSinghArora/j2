#!/usr/bin/env bash
# Assemble a self-contained, relocatable J2 install bundle
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
OUT="${1:?usage: mkbundle.sh <output-dir>}"
TRIPLE="aarch64-apple-darwin"
STAGE1="$ROOT/build/$TRIPLE/stage1"
CARGO="$ROOT/build/$TRIPLE/stage0/bin/cargo"
DRIVER="$ROOT/src/bin/j2/target/release/j2"

for p in "$STAGE1/bin/rustc" "$CARGO" "$DRIVER" "$ROOT/j2-vendor" "$ROOT/library/j2_runtime/src"; do
  [ -e "$p" ] || { echo "missing: $p" >&2; exit 1; }
done

echo "Assembling J2 bundle -> $OUT"
rm -rf "$OUT"; mkdir -p "$OUT/bin" "$OUT/library/j2_runtime"
echo "[1/7] driver";            cp "$DRIVER" "$OUT/bin/j2"
echo "[2/7] toolchain (~700M)"; cp -R "$STAGE1" "$OUT/toolchain"
echo "[3/7] cargo";             cp "$CARGO" "$OUT/toolchain/bin/cargo"
# [4] prune toolchain to what compiling needs
echo "[4/7] prune toolchain"
LLB="$OUT/toolchain/lib/rustlib/$TRIPLE/bin"
rm -f "$LLB"/opt "$LLB"/llc "$LLB"/llvm-nm "$LLB"/llvm-objdump "$LLB"/llvm-ar
rm -f "$OUT"/toolchain/bin/rustdoc
rm -f "$OUT"/toolchain/lib/librustc-*_rt.*.dylib
strip -S "$OUT"/toolchain/lib/librustc_driver-*.dylib 2>/dev/null || true
echo "[5/7] vendored deps";     cp -R "$ROOT/j2-vendor" "$OUT/j2-vendor"
echo "[6/7] j2_runtime source";  cp -R "$ROOT/library/j2_runtime/src" "$ROOT/library/j2_runtime/Cargo.toml" "$ROOT/library/j2_runtime/Cargo.lock" "$OUT/library/j2_runtime/"
# J2 is dual-licensed MIT OR Apache-2.0
echo "[7/7] licenses";          cp "$ROOT/deploy/LICENSE-MIT" "$ROOT/LICENSE-APACHE" "$ROOT/deploy/THIRD-PARTY-NOTICES.md" "$OUT/"
echo "DONE: $OUT ($(du -sh "$OUT" | cut -f1))"
