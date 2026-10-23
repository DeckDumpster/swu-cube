#!/bin/sh
# Tests for bin/swu-cube-check-launch: checksum mismatch and an unsupported platform are refused
# by name, a verified download is cached and exec'd, an unpinned launcher refuses.
set -eu

root=$(cd "$(dirname "$0")/.." && pwd)
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
fails=0

sum() { if command -v sha256sum >/dev/null 2>&1; then sha256sum "$1"; else shasum -a 256 "$1"; fi | cut -d ' ' -f 1; }

# The release dir holds a fake binary for this host's target; the launcher is a copy with pins.
case "$(uname -s)/$(uname -m)" in
    Darwin/arm64) target=aarch64-apple-darwin ;;
    Linux/x86_64) target=x86_64-unknown-linux-gnu ;;
    *) echo "skip: no release target for this host"; exit 0 ;;
esac
pin_var=$(printf '%s' "$target" | tr - _)
mkdir -p "$work/rel/v9.9.9"
printf '#!/bin/sh\necho "fake-check $*"\n' > "$work/rel/v9.9.9/swu-cube-check-$target"
good=$(sum "$work/rel/v9.9.9/swu-cube-check-$target")

launcher() { # launcher NAME PIN
    sed -e 's/^VERSION=.*/VERSION="9.9.9"/' -e "s/^SHA256_$pin_var=.*/SHA256_$pin_var=\"$2\"/" \
        "$root/bin/swu-cube-check-launch" > "$work/$1"
    chmod +x "$work/$1"
}

run() { # run LAUNCHER CACHE [PATH-prefix]; sets out and code
    code=0
    out=$(PATH="${3:+$3:}$PATH" CLAUDE_PLUGIN_DATA="$work/$2" SWU_CUBE_CHECK_RELEASE_BASE="file://$work/rel" \
        "$work/$1" mcp 2>&1) || code=$?
}

expect() { # expect NAME CODE-is-zero(0|1) PATTERN
    if { [ "$2" = 0 ] && [ "$code" -ne 0 ]; } || { [ "$2" = 1 ] && [ "$code" -eq 0 ]; } || ! printf '%s' "$out" | grep -q "$3"; then
        echo "FAIL $1: exit $code, output: $out"
        fails=$((fails + 1))
    else
        echo "ok $1"
    fi
}

launcher good "$good"
run good cache-good
expect "verified download is exec'd with its arguments" 0 "fake-check mcp"
[ -f "$work/cache-good/swu-cube-check-9.9.9-$target" ] || { echo "FAIL download not cached"; fails=$((fails + 1)); }
rm -f "$work/rel/v9.9.9/swu-cube-check-$target" # the second run must come from the cache
run good cache-good
expect "cached binary is exec'd without a download" 0 "fake-check mcp"
printf '%s\n' tampered >> "$work/cache-good/swu-cube-check-9.9.9-$target"
run good cache-good
expect "tampered cache is refused" 1 "checksum mismatch for cached"

printf '#!/bin/sh\necho "fake-check $*"\n' > "$work/rel/v9.9.9/swu-cube-check-$target"
launcher bad "$(printf 0%.0s $(seq 64))"
run bad cache-bad
expect "checksum mismatch is refused" 1 "checksum mismatch"
[ -z "$(ls "$work/cache-bad" 2>/dev/null | grep -v '^download')" ] || { echo "FAIL a mismatched binary was cached"; fails=$((fails + 1)); }

mkdir "$work/stub"
printf '#!/bin/sh\ncase "$1" in -s) echo Linux ;; -m) echo riscv64 ;; esac\n' > "$work/stub/uname"
chmod +x "$work/stub/uname"
run good cache-plat "$work/stub"
expect "unsupported platform is refused" 1 "unsupported platform Linux/riscv64"

launcher unpinned unpinned
run unpinned cache-unpinned
expect "unpinned launcher is refused" 1 "no release is pinned"

[ "$fails" -eq 0 ] || { echo "$fails failed"; exit 1; }
