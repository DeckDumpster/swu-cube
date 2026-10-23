#!/bin/sh
# Usage: scripts/pin-release.sh <version>   (e.g. 0.1.0 or v0.1.0)
# Downloads the release's SHA256SUMS and rewrites VERSION and the two SHA256_ lines in
# bin/swu-cube-check-launch. Refuses, changing nothing, if either binary is missing from it.
# SWU_CUBE_CHECK_RELEASE_BASE and SWU_CUBE_CHECK_LAUNCHER override the release URL base and the
# launcher path (the tests use them).
set -eu

die() {
    printf 'pin-release: %s\n' "$*" >&2
    exit 1
}

[ $# -eq 1 ] || die "usage: pin-release.sh <version>"
version=${1#v}
[ -n "$version" ] || die "empty version"

root=$(cd "$(dirname "$0")/.." && pwd)
launcher=${SWU_CUBE_CHECK_LAUNCHER:-$root/bin/swu-cube-check-launch}
base=${SWU_CUBE_CHECK_RELEASE_BASE:-https://github.com/DeckDumpster/swu-cube/releases/download}
url="$base/v$version/SHA256SUMS"

[ -f "$launcher" ] || die "no launcher at $launcher"
tmp=$(mktemp)
trap 'rm -f "$tmp" "$tmp.new"' EXIT
curl -fsSL -o "$tmp" "$url" || die "cannot download $url"

sum() { # sum TARGET: the checksum of swu-cube-check-TARGET, or die
    s=$(grep -E "^[0-9a-f]{64} [ *]?swu-cube-check-$1\$" "$tmp" | cut -d ' ' -f 1)
    [ -n "$s" ] || die "SHA256SUMS of v$version has no entry for swu-cube-check-$1"
    printf '%s' "$s"
}
darwin=$(sum aarch64-apple-darwin)
linux=$(sum x86_64-unknown-linux-gnu)

sed \
    -e "s/^VERSION=.*/VERSION=\"$version\"/" \
    -e "s/^SHA256_aarch64_apple_darwin=.*/SHA256_aarch64_apple_darwin=\"$darwin\"/" \
    -e "s/^SHA256_x86_64_unknown_linux_gnu=.*/SHA256_x86_64_unknown_linux_gnu=\"$linux\"/" \
    "$launcher" > "$tmp.new"
cat "$tmp.new" > "$launcher"
printf 'pin-release: pinned %s to v%s\n' "$launcher" "$version" >&2
