#!/usr/bin/env bash
# Verify the differential gate itself, without compiling test programs.
set -euo pipefail
cd "$(dirname "$0")/.."
work=$(mktemp -d "${TMPDIR:-/tmp}/j2-harness-test.XXXXXX")
trap 'rm -rf "$work"' EXIT
cat > "$work/driver" <<'DRIVER'
#!/usr/bin/env bash
mode=int
# The actual CLI uses presence of either flag to force compilation.
# Check this contract rather than treating a literal "0" as absent.
if [ "${J2_NO_NATIVE:-0}" != 1 ] && [ "${J2_FORCE_NATIVE:-0}" != 1 ] \
    && { [ "${J2_NO_NATIVE+x}" = x ] || [ "${J2_FORCE_NATIVE+x}" = x ]; }; then
  echo "interpreter mode inherited a force-compile flag" >&2
  exit 24
fi
if [ "${J2_NO_NATIVE:-0}" = 1 ]; then
  mode=dyn
elif [ "${J2_FORCE_NATIVE:-0}" = 1 ]; then
  if [ "${J2_PARALLEL:-1}" = 0 ]; then mode=nat; else mode=par; fi
fi
printf 'same output\n'
if [ "${J2_TEST_FAILURE_MODE:-}" = "$mode" ]; then
  echo "intentional $mode failure" >&2
  exit 23
fi
if [ "${J2_TEST_FAILURE_MODE:-}" = mismatch ] && [ "$mode" = par ]; then
  printf 'different\n'
fi
if [ "${J2_TEST_FAILURE_MODE:-}" = newline ] && [ "$mode" = par ]; then
  printf '\n'
fi
DRIVER
chmod +x "$work/driver"
J2_TEST_DRIVER="$work/driver" ./j2-tests/diff_harness.sh op_add > "$work/log"
for mode in dyn nat par int mismatch newline; do
  if J2_TEST_FAILURE_MODE="$mode" J2_TEST_DRIVER="$work/driver" \
      ./j2-tests/diff_harness.sh op_add > "$work/log" 2>&1; then
    echo "harness incorrectly accepted $mode failure"
    exit 1
  fi
  case "$mode" in
    dyn|nat|par|int) grep -q "intentional $mode failure" "$work/log" ;;
  esac
done
if J2_TEST_DRIVER="$work/driver" ./j2-tests/diff_harness.sh missing_v2_selftest > "$work/log" 2>&1; then
  echo "harness incorrectly skipped a missing test"
  exit 1
fi
echo "harness self-test: passed (four mode failures, output mismatch, newline mismatch, missing file)"
