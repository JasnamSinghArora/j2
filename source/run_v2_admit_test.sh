#!/usr/bin/env bash
# Three-way build + time for test_v2_admit.rs

set -euo pipefail

RUSTC="./build/aarch64-apple-darwin/stage1/bin/rustc"
SYSROOT="./build/aarch64-apple-darwin/stage1"
SRC="test_v2_admit.rs"

if [[ ! -x "$RUSTC" ]]; then
  echo "stage1 compiler not found at $RUSTC, build stage1 first" >&2
  exit 1
fi
if [[ ! -f "$SRC" ]]; then
  echo "$SRC not found in $(pwd)" >&2
  exit 1
fi

echo "=== build: baseline (no pass) ==="
"$RUSTC" --sysroot "$SYSROOT" -O "$SRC" -o custom_baseline

echo "=== build: legacy ==="
PARALLEL_LOWERING=1 \
  "$RUSTC" --sysroot "$SYSROOT" -O "$SRC" -o custom_legacy 2>&1 \
  | tee legacy_compile.log

echo "=== build: v2 default costs ==="
PARALLEL_LOWERING=1 PARALLEL_LOWERING_V2=1 \
PARALLEL_LOWERING_V2_DUMP=1 PARALLEL_LOWERING_STATS=1 \
  "$RUSTC" --sysroot "$SYSROOT" -O "$SRC" -o custom_v2 2>&1 \
  | tee v2_default_compile.log

echo "=== build: v2 aggressive (warm-pool) costs ==="
PARALLEL_LOWERING=1 PARALLEL_LOWERING_V2=1 \
PARALLEL_LOWERING_V2_DUMP=1 PARALLEL_LOWERING_STATS=1 \
PARALLEL_MARGIN_NS=0 \
PARALLEL_COST_DISPATCH_NS=100 \
PARALLEL_COST_HELPER_CALL_NS=10 \
PARALLEL_COST_MARSHAL_IN_NS=1 \
PARALLEL_COST_MARSHAL_OUT_NS=1 \
PARALLEL_COST_SYNC_NS=50 \
  "$RUSTC" --sysroot "$SYSROOT" -O "$SRC" -o custom_v2_aggr 2>&1 \
  | tee v2_aggr_compile.log

echo
echo "=== grep admit verdicts ==="
for log in legacy_compile.log v2_default_compile.log v2_aggr_compile.log; do
  echo "--- $log ---"
  grep -E '\[PAR(-V2|-LW)?\]|admit=' "$log" || true
done

echo
echo "=== timing: 5 runs per binary ==="
for bin in custom_baseline custom_legacy custom_v2 custom_v2_aggr; do
  if [[ ! -x "$bin" ]]; then
    echo "--- $bin: NOT BUILT ---"
    continue
  fi
  echo "--- $bin ---"
  for i in 1 2 3 4 5; do
    /usr/bin/time -p "./$bin" 2>&1 | tail -3
    echo
  done
done

echo
echo "Expected:"
echo "  custom_baseline: near-zero wall (LLVM folds the body)"
echo "  custom_legacy: slower: lowered unprofitable work, pays dispatch"
echo "  custom_v2: matches baseline: oracle (default) rejects"
echo "  custom_v2_aggr: look for [PAR-V2] admit=true in v2_aggr_compile.log"
