#!/usr/bin/env bash
# Differential correctness harness for J2

set -u
cd "$(dirname "$0")/.." || exit 2
J2="./src/bin/j2/target/release/j2"
[ -x "$J2" ] || { echo "build the driver first: (cd src/bin/j2 && cargo build --release)"; exit 2; }

# Curated deterministic, self-contained tests
DEFAULT_TESTS=(
  ml_matvec ml_kmeans ml_linreg ml_knn ml_nn ml_conv1d
  ml_argmin ml_axpy ml_binary ml_elemwise ml_grid_search ml_multidim
  ml_phase6 ml_phase7 ml_select ml_unary
  iter_sum general_smoke oop_smoke native_class native_class_mut
  hof_smoke class_hof_edge par_clamp_sqadd par_invoke give_shadow
  traffic_system realworld lambdas op_add generic_runtime generic_types
  bitset_demo combined error_classes purity_safe
)

# past give/shadow divergences fixed; keep tests green

if [ "$#" -gt 0 ]; then TESTS=("$@"); else TESTS=("${DEFAULT_TESTS[@]}"); fi

# strip timing noise to avoid false mismatches
normalize() { sed -E 's/ms=[0-9.eE+-]+/ms=X/g; s/[0-9]+\.[0-9]+(ms|s)\b/Xms/g'; }

run() { # mode file -> normalized stdout
  # interpreter-first default, so force native/dynamic via env
  case "$1" in
    dyn) J2_NO_NATIVE=1 J2_PARALLEL=0 $J2 --allow-all "j2-tests/$2.j2" 2>/dev/null ;;
    nat) J2_FORCE_NATIVE=1 J2_PARALLEL=0 $J2 --allow-all "j2-tests/$2.j2" 2>/dev/null ;;
    par) J2_FORCE_NATIVE=1 $J2 --allow-all "j2-tests/$2.j2" 2>/dev/null ;;
    # AST interpreter (`j2 run`), now the default
    int) $J2 --allow-all run "j2-tests/$2.j2" 2>/dev/null ;;
  esac | normalize
}

pass=0; fail=0; failed=()
for t in "${TESTS[@]}"; do
  [ -f "j2-tests/$t.j2" ] || { echo "SKIP $t (no file)"; continue; }
  d=$(run dyn "$t"); n=$(run nat "$t"); p=$(run par "$t"); i=$(run int "$t")
  if [ "$d" = "$n" ] && [ "$n" = "$p" ] && [ "$p" = "$i" ]; then
    echo "PASS  $t"; pass=$((pass+1))
  else
    echo "FAIL  $t"; fail=$((fail+1)); failed+=("$t")
    [ "$d" != "$n" ] && echo "   dyn≠nat:" && diff <(printf '%s' "$d") <(printf '%s' "$n") | head -4
    [ "$n" != "$p" ] && echo "   nat≠par:" && diff <(printf '%s' "$n") <(printf '%s' "$p") | head -4
    [ "$p" != "$i" ] && echo "   compiled≠interp:" && diff <(printf '%s' "$p") <(printf '%s' "$i") | head -4
  fi
done

echo "-----------------------------------------------"
echo "differential: $pass passed, $fail failed"
[ "$fail" -eq 0 ] || { echo "MISMATCHES: ${failed[*]}"; exit 1; }
