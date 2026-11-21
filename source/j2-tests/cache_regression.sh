#!/usr/bin/env bash
# End-to-end cache regression: imports are part of the compiled program identity.
set -euo pipefail
cd "$(dirname "$0")/.."
J2="${J2_TEST_DRIVER:-./src/bin/j2/target/release/j2}"
work=$(mktemp -d "${TMPDIR:-/tmp}/j2-cache-regression.XXXXXX")
serial_pid=
parallel_pid=
cleanup() {
  # Release either child even when a readiness/output check fails.
  : > "$work/release"
  for pid in "$serial_pid" "$parallel_pid"; do
    if [ -n "$pid" ]; then wait "$pid" 2>/dev/null || true; fi
  done
  rm -rf "$work"
}
trap cleanup EXIT
mkdir -p "$work/a" "$work/b"
printf 'import "module.j2"\nprint(answer)\n' > "$work/a/main.j2"
cp "$work/a/main.j2" "$work/b/main.j2"
printf 'import "nested.j2"\n' > "$work/a/module.j2"
printf 'answer = 41\n' > "$work/a/nested.j2"
printf 'answer = 99\n' > "$work/b/module.j2"
check() {
  "$J2" build "$1" -o "$work/program"
  local got
  got=$("$work/program")
  if [ "$got" != "$2" ]; then
    echo "cache regression: got [$got], expected [$2]"
    exit 1
  fi
}
check "$work/a/main.j2" 41
check "$work/a/main.j2" 41
printf 'answer = 42\n' > "$work/a/nested.j2"
check "$work/a/main.j2" 42
check "$work/b/main.j2" 99
rm "$work/a/nested.j2"
if "$J2" build "$work/a/main.j2" -o "$work/program" > "$work/error" 2>&1; then
  echo "cache regression: cached program hid a deleted import"
  exit 1
fi
grep -q 'cannot read' "$work/error"

# Two CLI runs of the same source must own separate executables. Previously
# both copied to /tmp/j_<source>, overwriting an executable still in use and
# causing silent signal exits on macOS. Prewarm both compiler modes, then use
# readiness/release files to guarantee overlap without a timing-dependent nap.
cat > "$work/concurrent.j2" <<'PROGRAM'
ready = proc.env("J2_CACHE_TEST_READY")
release = proc.env("J2_CACHE_TEST_RELEASE")
fs.write_file(ready, "ready")
started = time.now()
repeat not fs.exists(release) {
    assert time.elapsed_ms(started) < 10000, "concurrent run was never released"
    async.sleep_ms(10)
}
values = collect(1..33000)
mapped = map(values, _ + 1)
assert_eq(sum(mapped), 544549500)
print(proc.env("J2_PARALLEL"))
PROGRAM
for mode in 0 1; do
  J2_NO_CACHE=0 J2_FORCE_NATIVE=1 J2_NO_NATIVE=0 J2_PARALLEL="$mode" \
    "$J2" --allow-all build "$work/concurrent.j2" -o "$work/warm-$mode"
done
wait_ready() {
  local mode=$1 pid=$2 attempt
  for ((attempt=0; attempt<1000; attempt++)); do
    if [ -f "$work/$mode.ready" ]; then return 0; fi
    if ! kill -0 "$pid" 2>/dev/null; then
      echo "cache regression: concurrent mode $mode exited before becoming ready"
      cat "$work/$mode.err"
      return 1
    fi
    sleep 0.01
  done
  echo "cache regression: concurrent mode $mode did not become ready"
  cat "$work/$mode.err"
  return 1
}
J2_NO_CACHE=0 J2_FORCE_NATIVE=1 J2_NO_NATIVE=0 J2_PARALLEL=0 \
  J2_CACHE_TEST_READY="$work/0.ready" J2_CACHE_TEST_RELEASE="$work/release" \
  "$J2" --allow-all "$work/concurrent.j2" > "$work/0.out" 2> "$work/0.err" &
serial_pid=$!
wait_ready 0 "$serial_pid"
J2_NO_CACHE=0 J2_FORCE_NATIVE=1 J2_NO_NATIVE=0 J2_PARALLEL=1 \
  J2_CACHE_TEST_READY="$work/1.ready" J2_CACHE_TEST_RELEASE="$work/release" \
  "$J2" --allow-all "$work/concurrent.j2" > "$work/1.out" 2> "$work/1.err" &
parallel_pid=$!
wait_ready 1 "$parallel_pid"
: > "$work/release"
serial_status=0
parallel_status=0
wait "$serial_pid" || serial_status=$?
wait "$parallel_pid" || parallel_status=$?
serial_pid=
parallel_pid=
if [ "$serial_status" -ne 0 ] || [ "$parallel_status" -ne 0 ]; then
  echo "cache regression: concurrent exits serial=$serial_status parallel=$parallel_status"
  cat "$work/0.err" "$work/1.err"
  exit 1
fi
for mode in 0 1; do
  printf '%s\n' "$mode" > "$work/expected"
  if ! cmp -s "$work/expected" "$work/$mode.out"; then
    echo "cache regression: unexpected output from concurrent mode $mode"
    cat "$work/$mode.out" "$work/$mode.err"
    exit 1
  fi
done
echo "cache regression: passed (cache hit, transitive edit, different directory, deleted import, concurrent serial/parallel runs)"
