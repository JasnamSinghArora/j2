#!/usr/bin/env bash
# J2 correctness CI gate
set -u
cd "$(dirname "$0")/.." || exit 2
J2="./src/bin/j2/target/release/j2"
fail=0

echo "== 1. build driver =="
( cd src/bin/j2 && cargo build --release ) || { echo "BUILD FAILED"; exit 1; }

echo "== 2. parse/lower sweep =="
swept=0
# trusted gate so capability-gated builtins aren't denied
export J2_TRUSTED=1 J2_ALLOW_FS=1 J2_ALLOW_PROC=1 J2_ALLOW_NET=1
for f in j2-tests/*.j2; do
  swept=$((swept+1))
  out=$($J2 emit-native "$f" 2>&1)
  if echo "$out" | grep -qiE "panicked|parse error|error\[SyntaxError"; then
    echo "  LOWER FAIL: $f"; echo "$out" | grep -iE "panicked|error\[" | head -1; fail=1
  fi
done
echo "  swept $swept files (clean unless LOWER FAIL above)"

echo "== 3. differential harness =="
./j2-tests/diff_harness.sh || fail=1

echo "== 4. reference checksum regression =="
# force native to catch native-codegen drift
check() { local got; got=$(J2_FORCE_NATIVE=1 $J2 "j2-tests/$1.j2" 2>/dev/null | grep -oE "$2" | head -1)
  if [ "$got" = "$3" ]; then echo "  OK    $1 ($got)"; else echo "  DRIFT $1: got [$got] want [$3]"; fail=1; fi; }
check ml_matvec     'checksum=[0-9.]+'   'checksum=479989'
check ml_kmeans     'inertia=[0-9.]+'    'inertia=45.83069701416572'
check ml_linreg     'final_loss=[0-9.]+' 'final_loss=1.0198697405471906'
check ml_knn        'checksum=[0-9.]+'   'checksum=3.7284710966291077'
check ml_nn         'final_loss=[0-9.]+' 'final_loss=0.2926843250109515'
check ml_conv1d     'checksum=[0-9.]+'   'checksum=6794968.5'
check iter_sum      'sum=[0-9]+'         'sum=8000002000000'
check general_smoke 'sha256=[a-f0-9]+'   'sha256=a635ea493c5b3127'

echo "-----------------------------------------------"
if [ "$fail" -eq 0 ]; then echo "CI: ALL GREEN"; else echo "CI: FAILURES"; exit 1; fi
