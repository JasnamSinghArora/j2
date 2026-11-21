#!/usr/bin/env bash
# Differential correctness harness for J2.
#
# For each test, run it four ways and assert byte-identical stdout:
#   (dyn) J2_NO_NATIVE=1 J2_PARALLEL=0  - pure dynamic interpreter (the oracle)
#   (nat) J2_PARALLEL=0                - typed native lowering, serial
#   (par) <default>                   - typed native lowering, auto-parallelized
#
# This guards J2's central invariant: native and native+parallel output must
# never diverge from the dynamic interpreter. It would have caught the matvec
# matcher miscompile. Volatile fields (timings like `ms=...`) are normalized
# out before comparison. Run trusted (--allow-all) so capability-gated tests
# execute in all four modes.
#
# Usage: j2-tests/diff_harness.sh [test1.j2 test2.j2 ...]   (defaults to the
#        curated deterministic, self-contained set below).

set -uo pipefail
cd "$(dirname "$0")/.." || exit 2
J2="${J2_TEST_DRIVER:-./src/bin/j2/target/release/j2}"
[ -x "$J2" ] || { echo "build the driver first: (cd src/bin/j2 && cargo build --release)"; exit 2; }

# Curated deterministic, self-contained tests (no stdin/file-arg/network/clock
# dependence). Timing fields are normalized; rand-based ML tests are seeded so
# their numeric output is stable across modes.
DEFAULT_TESTS=(
  ml_matvec ml_kmeans ml_linreg ml_knn ml_nn ml_conv1d
  ml_argmin ml_axpy ml_binary ml_elemwise ml_grid_search ml_multidim
  ml_phase6 ml_phase7 ml_select ml_unary
  iter_sum general_smoke oop_smoke native_class native_class_mut
  hof_smoke class_hof_edge par_clamp_sqadd par_invoke give_shadow
  traffic_system realworld lambdas op_add generic_runtime generic_types
  bitset_demo combined error_classes purity_safe
  v2_syntax v2_parallel v2_errors v2_shadowing
  v2_capture_parallel v2_loop_parallel
)

# History: this harness originally found two dynamic-interpreter divergences -
# `give` (early return) was encoded as an uncaught error string, and a
# function-local binding could not shadow a same-named top-level constant.
# Both are now FIXED (give → GiveSignal control-flow + give-slot caught in the
# body wrapper; per-scope `locals` tracking in __bind), so native_class,
# par_invoke and give_shadow are now in the default set and must stay green.

if [ "$#" -gt 0 ]; then TESTS=("$@"); else TESTS=("${DEFAULT_TESTS[@]}"); fi

# Strip volatile fields so timing noise doesn't cause false mismatches.
normalize() { sed -E 's/ms=[0-9.eE+-]+/ms=X/g; s/[0-9]+\.[0-9]+(ms|s)\b/Xms/g'; }

# Retain diagnostics and compare complete output (including trailing newlines).
work=$(mktemp -d "${TMPDIR:-/tmp}/j2-diff.XXXXXX") || exit 2
trap 'rm -rf "$work"' EXIT

run() { # mode file -> normalized stdout; pipefail preserves the driver's status
  # Set each mode explicitly so the caller's environment cannot turn all four
  # runs into the same execution path.
  case "$1" in
    dyn) J2_FORCE_NATIVE=1 J2_NO_NATIVE=1 J2_PARALLEL=0 "$J2" --allow-all "j2-tests/$2.j2" ;;
    nat) J2_FORCE_NATIVE=1 J2_NO_NATIVE=0 J2_PARALLEL=0 "$J2" --allow-all "j2-tests/$2.j2" ;;
    par) J2_FORCE_NATIVE=1 J2_NO_NATIVE=0 J2_PARALLEL=1 "$J2" --allow-all "j2-tests/$2.j2" ;;
    int) env -u J2_FORCE_NATIVE -u J2_NO_NATIVE J2_PARALLEL=1 "$J2" --allow-all run "j2-tests/$2.j2" ;;
  esac | normalize
}

pass=0; fail=0; failed=()
for t in "${TESTS[@]}"; do
  # Accept the documented file arguments as well as the historical bare names.
  t=${t#j2-tests/}
  t=${t%.j2}
  if [ ! -f "j2-tests/$t.j2" ]; then
    echo "FAIL  $t (no file)"
    fail=$((fail+1)); failed+=("$t")
    continue
  fi
  run_failed=0
  for mode in dyn nat par int; do
    if run "$mode" "$t" >"$work/$mode.out" 2>"$work/$mode.err"; then
      :
    else
      status=$?
      echo "  $t: $mode exited $status"
      sed -n '1,12p' "$work/$mode.err"
      run_failed=1
    fi
  done
  if [ "$run_failed" -ne 0 ]; then
    echo "FAIL  $t (execution failed)"
    fail=$((fail+1)); failed+=("$t")
    continue
  fi
  if cmp -s "$work/dyn.out" "$work/nat.out" \
    && cmp -s "$work/nat.out" "$work/par.out" \
    && cmp -s "$work/par.out" "$work/int.out"; then
    echo "PASS  $t"; pass=$((pass+1))
  else
    echo "FAIL  $t"; fail=$((fail+1)); failed+=("$t")
    for mode in nat par int; do
      if ! cmp -s "$work/dyn.out" "$work/$mode.out"; then
        echo "   dyn≠$mode:"
        diff -u "$work/dyn.out" "$work/$mode.out" | sed -n '1,12p'
      fi
    done
  fi
done

echo "-----------------------------------------------"
echo "differential: $pass passed, $fail failed"
[ "$fail" -eq 0 ] || { echo "FAILURES: ${failed[*]}"; exit 1; }
