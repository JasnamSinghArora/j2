#!/usr/bin/env bash
# runs dispatch microbench, workload sweep, compiler-driven benches

set -euo pipefail

RUSTC="./build/host/stage1/bin/rustc"
SYSROOT="./build/host/stage1"

if [[ ! -x "$RUSTC" ]]; then
  echo "stage1 compiler not found, run ./x.py build library --stage 1 first" >&2
  exit 1
fi

OUTDIR="${1:-bench_results}"
mkdir -p "$OUTDIR"

echo "=== part 1: dispatch microbench ==="
"$RUSTC" --sysroot "$SYSROOT" -O bench_dispatch.rs -o /tmp/bench_dispatch
/tmp/bench_dispatch | tee "$OUTDIR/01_dispatch.txt"

echo
echo "=== part 1b: reduction idiom (parallel_reduce_*) ==="
"$RUSTC" --sysroot "$SYSROOT" -O bench_reduce.rs -o /tmp/bench_reduce
/tmp/bench_reduce | tee "$OUTDIR/01b_reduce.txt"

echo
echo "=== part 2: workload sweep ==="
"$RUSTC" --sysroot "$SYSROOT" -O bench_workload.rs -o /tmp/bench_workload
/tmp/bench_workload | tee "$OUTDIR/02_workload.txt"

echo
echo "=== part 3a: oracle decisions on test_v2_admit.rs ==="
PARALLEL_LOWERING=1 PARALLEL_LOWERING_V2_DUMP=1 \
  "$RUSTC" --sysroot "$SYSROOT" -O test_v2_admit.rs -o /tmp/test_v2_admit_default \
  2>&1 | tee "$OUTDIR/03a_oracle_default.txt"

echo
echo "=== part 3b: oracle decisions on test.rs (loop-heavy) ==="
PARALLEL_LOWERING=1 PARALLEL_LOWERING_V2_DUMP=1 \
  "$RUSTC" --sysroot "$SYSROOT" -O test.rs -o /tmp/test_loop_default \
  2>&1 | tee "$OUTDIR/03b_oracle_loop.txt"

echo
echo "=== part 4: correctness, baseline vs lowered (must match) ==="
"$RUSTC" --sysroot "$SYSROOT" -O test_correctness.rs -o /tmp/test_corr_baseline 2>&1 | tail -1
PARALLEL_LOWERING=1 PARALLEL_MARGIN_NS=0 PARALLEL_COST_DISPATCH_NS=0 \
PARALLEL_COST_HELPER_CALL_NS=0 PARALLEL_COST_MARSHAL_IN_NS=0 \
PARALLEL_COST_MARSHAL_OUT_NS=0 PARALLEL_COST_SYNC_NS=0 \
  "$RUSTC" --sysroot "$SYSROOT" -O test_correctness.rs -o /tmp/test_corr_lowered 2>&1 | tail -3

set +e
/tmp/test_corr_baseline; B=$?
/tmp/test_corr_lowered; L=$?
PAR_RUNTIME_FORCE_PARALLEL=1 /tmp/test_corr_lowered; LF=$?
set -e

{
  echo "baseline_exit=$B"
  echo "lowered_exit=$L"
  echo "lowered_force_parallel_exit=$LF"
  if [[ "$B" == "$L" && "$L" == "$LF" ]]; then
    echo "RESULT: PASS, all three exits match"
  else
    echo "RESULT: FAIL, exits diverge"
  fi
} | tee "$OUTDIR/04_correctness.txt"

echo
echo "=== summary ==="
echo "results saved to: $OUTDIR/"
ls -la "$OUTDIR"
