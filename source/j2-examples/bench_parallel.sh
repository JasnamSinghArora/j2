#!/usr/bin/env bash
# Measure J2's auto-parallel speedup
set -euo pipefail
J2="${J2:-j}"
here="$(cd "$(dirname "$0")" && pwd)"
run() { "$J2" --allow-all "$1" 2>/dev/null | grep -oE "ms=[0-9.]+|[0-9.]+ ms" | grep -oE "[0-9.]+" | tail -1; }
printf "%-26s %-12s %-12s %s\n" "example" "parallel(ms)" "serial(ms)" "speedup"
for f in parallel_montecarlo parallel_integrate parallel_matvec; do
  p=$(J2_FORCE_NATIVE=1 "$J2" --allow-all "$here/$f.j2" 2>/dev/null | grep -oE "[0-9.]+ ms|ms=[0-9.]+" | grep -oE "[0-9.]+" | tail -1)
  s=$(J2_PARALLEL=0 J2_FORCE_NATIVE=1 "$J2" --allow-all "$here/$f.j2" 2>/dev/null | grep -oE "[0-9.]+ ms|ms=[0-9.]+" | grep -oE "[0-9.]+" | tail -1)
  sp=$(awk -v s="$s" -v p="$p" 'BEGIN{ if (p>0) printf "%.2fx", s/p; else print "n/a" }')
  printf "%-26s %-12s %-12s %s\n" "$f" "$p" "$s" "$sp"
done
