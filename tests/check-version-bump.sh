#!/bin/sh
# Tests for scripts/check-version-bump.sh against fixture repos: a changed skill with an unchanged
# version fails; with a bumped version it passes; mismatched Cargo and plugin versions fail.
set -eu

root=$(cd "$(dirname "$0")/.." && pwd)
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
fails=0
fail() { echo "FAIL: $*"; fails=$((fails + 1)); }

repo=$work/repo
mkdir -p "$repo/.claude-plugin" "$repo/skills/s" "$repo/docs"
printf '[workspace.package]\nversion = "1.0.0"\n' > "$repo/Cargo.toml"
printf '{\n  "name": "x",\n  "version": "1.0.0"\n}\n' > "$repo/.claude-plugin/plugin.json"
echo one > "$repo/skills/s/SKILL.md"
echo one > "$repo/docs/README"
git -C "$repo" init -q -b main
git -C "$repo" add -A
git -C "$repo" -c user.name=t -c user.email=t@t commit -qm base
export SWU_CUBE_ROOT=$repo
check() { sh "$root/scripts/check-version-bump.sh" main 2>"$work/err"; }
reset() { git -C "$repo" checkout -q -- . && git -C "$repo" clean -qfd; }

check || fail "no change refused"

echo two > "$repo/docs/README"
check || fail "non-plugin change refused"
reset

echo two > "$repo/skills/s/SKILL.md"
if check; then fail "changed skill with unchanged version accepted"; fi
grep -q 'bump' "$work/err" || fail "no bump message"

sed -i 's/1\.0\.0/1.1.0/' "$repo/Cargo.toml" "$repo/.claude-plugin/plugin.json"
check || fail "changed skill with bumped version refused"

sed -i 's/1\.1\.0/1.2.0/' "$repo/Cargo.toml"
if check; then fail "mismatched versions accepted"; fi
grep -q 'make them equal' "$work/err" || fail "no mismatch message"
reset

mkdir -p "$repo/bin"; echo x > "$repo/bin/launch"; git -C "$repo" add bin
if check; then fail "changed bin with unchanged version accepted"; fi

[ "$fails" = 0 ] || exit 1
echo "check-version-bump tests passed"
