#!/usr/bin/env bash
# J2 correctness CI gate. Runs the full guard in one command:
#   1. build the driver
#   2. parse/lower sweep over every j-test (no SyntaxError/panic/broken-Rust)
#   3. differential harness: dynamic-interp ≡ native-serial ≡ native-parallel
#   4. reference-checksum regression (catches a drift where all modes change
#      together - the harness alone wouldn't, since it only compares modes)
#   5. cache invalidation and differential-harness regression checks
# Exits non-zero on any failure. Intended for CI and pre-commit.
set -uo pipefail
cd "$(dirname "$0")/.." || exit 2
J2="./src/bin/j2/target/release/j2"
fail=0

echo "== 1. build driver =="
( cd src/bin/j2 && cargo build --release ) || { echo "BUILD FAILED"; exit 1; }

echo "== 2. parse/lower sweep =="
swept=0
# Trusted local gate: allow `rust{}` to compile and capability-gated builtins
# (fs/proc/net) to run, so checksum tests like general_smoke (which hashes
# files) aren't denied.
export J2_TRUSTED=1 J2_ALLOW_FS=1 J2_ALLOW_PROC=1 J2_ALLOW_NET=1
for f in j2-tests/*.j2; do
  swept=$((swept+1))
  if out=$("$J2" emit-native "$f" 2>&1); then
    :
  else
    echo "  LOWER FAIL: $f"
    printf '%s\n' "$out" | sed -n '1,12p'
    fail=1
  fi
done
echo "  swept $swept files (clean unless LOWER FAIL above)"

echo "== 3. differential harness =="
./j2-tests/diff_harness.sh || fail=1

echo "== 4. reference checksum regression =="
# Force native (j2 FILE is interpreter-first by default now); this gate's job is
# to catch NATIVE-codegen drift, so it must exercise the compiled path.
check() {
  local output got
  if ! output=$(J2_FORCE_NATIVE=1 "$J2" "j2-tests/$1.j2" 2>&1); then
    echo "  ERROR $1 (execution failed)"
    printf '%s\n' "$output" | sed -n '1,12p'
    fail=1
    return
  fi
  got=$(printf '%s\n' "$output" | grep -oE "$2" | sed -n '1p')
  if [ "$got" = "$3" ]; then
    echo "  OK    $1 ($got)"
  else
    echo "  DRIFT $1: got [$got] want [$3]"
    fail=1
  fi
}
check ml_matvec     'checksum=[0-9.]+'   'checksum=479989'
check ml_kmeans     'inertia=[0-9.]+'    'inertia=45.83069701416572'
check ml_linreg     'final_loss=[0-9.]+' 'final_loss=1.0198697405471906'
check ml_knn        'checksum=[0-9.]+'   'checksum=3.7284710966291077'
check ml_nn         'final_loss=[0-9.]+' 'final_loss=0.2926843250109515'
check ml_conv1d     'checksum=[0-9.]+'   'checksum=6794968.5'
check iter_sum      'sum=[0-9]+'         'sum=8000002000000'
check general_smoke 'sha256=[a-f0-9]+'   'sha256=a635ea493c5b3127'

echo "== 5. cache and harness regressions =="
./j2-tests/cache_regression.sh || fail=1
./j2-tests/harness_selftest.sh || fail=1

echo "-----------------------------------------------"
if [ "$fail" -eq 0 ]; then echo "CI: ALL GREEN"; else echo "CI: FAILURES"; exit 1; fi
