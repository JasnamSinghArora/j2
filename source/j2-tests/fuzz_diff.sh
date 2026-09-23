#!/usr/bin/env bash
# serial 4-way differential; shared target dir uncontended
J2="${J2:-./src/bin/j2/target/release/j2}"
norm() { sed -E 's/ms=[0-9.eE+-]+/ms=X/g'; }
runmode() {
  case "$1" in
    dyn) timeout 120 env J2_NO_NATIVE=1 J2_PARALLEL=0 "$J2" --allow-all "$2" 2>/dev/null ;;
    nat) timeout 120 env J2_FORCE_NATIVE=1 J2_PARALLEL=0 "$J2" --allow-all "$2" 2>/dev/null ;;
    par) timeout 120 env J2_FORCE_NATIVE=1 "$J2" --allow-all "$2" 2>/dev/null ;;
    int) timeout 120 "$J2" --allow-all run "$2" 2>/dev/null ;;
  esac | norm
}
pass=0; fail=0; failed=()
for f in j2-tests/fuzz/*.j2; do
  b=$(basename "$f")
  d=$(runmode dyn "$f"); n=$(runmode nat "$f"); p=$(runmode par "$f"); i=$(runmode int "$f")
  if [ "$d" = "$n" ] && [ "$n" = "$p" ] && [ "$p" = "$i" ]; then
    pass=$((pass+1)); echo "OK $b"
  else
    fail=$((fail+1)); failed+=("$b")
    echo "DIVERGE $b"
    [ "$d" != "$i" ] && { echo "  --- dyn(oracle) vs interp ---"; diff <(printf '%s' "$d") <(printf '%s' "$i") | head -8; }
    [ "$d" != "$n" ] && { echo "  --- dyn vs nat ---"; diff <(printf '%s' "$d") <(printf '%s' "$n") | head -8; }
    [ "$n" != "$p" ] && { echo "  --- nat vs par ---"; diff <(printf '%s' "$n") <(printf '%s' "$p") | head -8; }
  fi
done
echo "======================================"
echo "TOTAL pass=$pass fail=$fail"
[ ${#failed[@]} -gt 0 ] && echo "FAILED: ${failed[*]}"
echo "FUZZ_DIFF_DONE"
