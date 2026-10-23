#!/bin/sh
# Tests for scripts/pin-release.sh against a fixture SHA256SUMS: it rewrites exactly the three
# pinned lines, and refuses (changing nothing) when a target is missing.
set -eu

root=$(cd "$(dirname "$0")/.." && pwd)
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
fails=0
fail() { echo "FAIL: $*"; fails=$((fails + 1)); }

a=$(printf 'a%.0s' $(seq 64))
b=$(printf 'b%.0s' $(seq 64))
mkdir -p "$work/rel/v1.2.3" "$work/rel/v1.2.4"
printf '%s  swu-cube-check-aarch64-apple-darwin\n%s  swu-cube-check-x86_64-unknown-linux-gnu\n' "$a" "$b" > "$work/rel/v1.2.3/SHA256SUMS"
printf '%s  swu-cube-check-aarch64-apple-darwin\n' "$a" > "$work/rel/v1.2.4/SHA256SUMS"

export SWU_CUBE_CHECK_RELEASE_BASE="file://$work/rel"
export SWU_CUBE_CHECK_LAUNCHER="$work/launch"
cp "$root/bin/swu-cube-check-launch" "$work/launch"

sh "$root/scripts/pin-release.sh" v1.2.3 2>/dev/null || fail "pin v1.2.3 refused"
changed=$(diff "$root/bin/swu-cube-check-launch" "$work/launch" | grep -c '^>' || true)
[ "$changed" = 3 ] || fail "expected exactly 3 changed lines, got $changed"
grep -qx 'VERSION="1.2.3"' "$work/launch" || fail "VERSION not pinned"
grep -qx "SHA256_aarch64_apple_darwin=\"$a\"" "$work/launch" || fail "darwin sum not pinned"
grep -qx "SHA256_x86_64_unknown_linux_gnu=\"$b\"" "$work/launch" || fail "linux sum not pinned"

cp "$work/launch" "$work/before"
if sh "$root/scripts/pin-release.sh" 1.2.4 2>"$work/err"; then fail "missing target accepted"; fi
grep -q 'swu-cube-check-x86_64-unknown-linux-gnu' "$work/err" || fail "refusal does not name the target"
cmp -s "$work/before" "$work/launch" || fail "launcher changed on a refusal"
if sh "$root/scripts/pin-release.sh" 9.9.9 2>/dev/null; then fail "missing release accepted"; fi

[ "$fails" = 0 ] && echo "pin-release tests passed"
exit "$fails"
